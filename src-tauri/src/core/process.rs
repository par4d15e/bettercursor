//! bettercursor process detection — per-layer write guards for L2/L3 sync.
//!
//! #84: refuse writes that race with live processes holding the same SQLite
//! file. Guards are **split by layer** (2026-07-05):
//!
//! - **L3** (`state.vscdb`): block on Cursor Desktop + `cursor-server` only.
//!   `cursor-agent` / `cursor-agent-worker` do not open global state.vscdb.
//! - **L2** (`store.db`): block only when a non-worker `cursor-agent` process
//!   holds **this session's** `store.db` (via `/proc/<pid>/fd` on Linux).
//!
//! Detection: `pgrep -af <pattern>` on Linux/macOS. Without `pgrep` we
//! conservatively report no blockers so the tmpdir-copy + atomic_rename path
//! still runs.

use std::path::Path;
use std::process::Command;

/// Cursor Desktop / remote server — holders of `globalStorage/state.vscdb`.
const L3_PATTERNS: &[&str] = &[
    "Cursor --type=",
    "/Cursor --updated",
    "Cursor-bin",
    "cursor-server",
];

/// CLI agent pattern (filtered: workers excluded; L2 further scoped per session).
const L2_AGENT_PATTERN: &str = "cursor-agent";

/// Legacy union: all processes that block *any* layer write. Used by
/// `delete_session` preflight when removing L2 (conservative summary).
pub fn cursor_processes_running() -> Vec<String> {
    let mut all = layer3_write_blocked();
    for line in layer2_write_blocked_any_agent() {
        if !all.contains(&line) {
            all.push(line);
        }
    }
    all
}

/// Processes that would race a **Layer 3** `state.vscdb` write.
pub fn layer3_write_blocked() -> Vec<String> {
    let mut matches = Vec::new();
    for pat in L3_PATTERNS {
        collect_pgrep_lines(pat, &mut matches);
    }
    matches
}

/// Any non-worker `cursor-agent` / `agent` CLI (for legacy `cursor_running`).
fn layer2_write_blocked_any_agent() -> Vec<String> {
    let mut matches = Vec::new();
    collect_pgrep_lines(L2_AGENT_PATTERN, &mut matches);
    matches.retain(|line| !is_agent_worker(line));
    matches
}

/// Processes that would race a **Layer 2** write for `uuid` under `cwd`.
///
/// Only returns blockers that hold this session's `store.db` or reference
/// `uuid` on the command line (`--resume=<uuid>`). Unrelated CLI sessions
/// do not block.
pub fn layer2_write_blocked(uuid: &str, cwd: &str) -> Vec<String> {
    if cwd.trim().is_empty() || uuid.is_empty() {
        return Vec::new();
    }
    let store_db = super::paths::store_db_for(cwd, uuid);
    let mut blockers = Vec::new();
    let mut candidates = Vec::new();
    collect_pgrep_lines(L2_AGENT_PATTERN, &mut candidates);
    for line in candidates {
        if is_agent_worker(&line) {
            continue;
        }
        let Some(pid) = parse_pid(&line) else {
            continue;
        };
        if command_line_targets_session(&line, uuid) || process_holds_file(pid, &store_db) {
            blockers.push(line);
        }
    }
    blockers
}

fn collect_pgrep_lines(pattern: &str, out: &mut Vec<String>) {
    let output = match Command::new("pgrep").args(["-af", pattern]).output() {
        Ok(o) if o.status.success() => o,
        _ => return,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.contains("pgrep") && line.contains(pattern) {
            continue;
        }
        if !out.contains(&line.to_string()) {
            out.push(line.to_string());
        }
    }
}

/// Background workers only write their own log — never `store.db` / `state.vscdb`.
fn is_agent_worker(line: &str) -> bool {
    line.contains("cursor-agent-worker") || line.contains(" worker start ")
}

fn parse_pid(line: &str) -> Option<u32> {
    line.split_whitespace().next()?.parse().ok()
}

fn command_line_targets_session(line: &str, uuid: &str) -> bool {
    line.contains(uuid)
        || line.contains(&format!("--resume={uuid}"))
        || line.contains(&format!("--resume {uuid}"))
}

/// Best-effort: true when `pid` has `target` open (Linux `/proc/<pid>/fd`).
#[cfg(unix)]
fn process_holds_file(pid: u32, target: &Path) -> bool {
    let fd_dir = format!("/proc/{pid}/fd");
    let Ok(entries) = std::fs::read_dir(&fd_dir) else {
        return false;
    };
    let target = target.canonicalize().unwrap_or_else(|_| target.to_path_buf());
    for entry in entries.flatten() {
        let Ok(link) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let resolved = link.canonicalize().unwrap_or(link);
        if resolved == target {
            return true;
        }
    }
    false
}

#[cfg(not(unix))]
fn process_holds_file(_pid: u32, _target: &Path) -> bool {
    // Without /proc, fall back to command-line uuid match only.
    false
}

fn format_lock_skip(prefix: &str, blockers: &[String]) -> String {
    if blockers.is_empty() {
        return String::new();
    }
    format!(
        "{prefix}({} proc, e.g. {})",
        blockers.len(),
        truncate_example(&blockers[0])
    )
}

fn truncate_example(s: &str) -> String {
    const MAX: usize = 120;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(MAX).collect::<String>())
    }
}

/// Shared skip-reason formatter for sync reports.
pub fn l3_lock_skip_reason(blockers: &[String]) -> String {
    format_lock_skip("l3_locked", blockers)
}

/// Shared skip-reason formatter for sync reports.
pub fn l2_lock_skip_reason(blockers: &[String]) -> String {
    format_lock_skip("l2_locked", blockers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pgrep_for(pattern: &str) -> Vec<String> {
        let mut out = Vec::new();
        collect_pgrep_lines(pattern, &mut out);
        out
    }

    #[test]
    fn cursor_processes_running_returns_vec() {
        let _ = cursor_processes_running();
    }

    #[test]
    fn layer3_write_blocked_returns_vec() {
        let _ = layer3_write_blocked();
    }

    #[test]
    fn pgrep_for_filters_empty_pattern() {
        let matches = pgrep_for("this_pattern_definitely_does_not_exist_zzzz");
        assert!(matches.is_empty(), "expected no matches, got {matches:?}");
    }

    #[test]
    fn is_agent_worker_detects_worker_start() {
        let line = "370767 cursor-agent worker start --worker-dir /home/eric/workspace/bettercursor";
        assert!(is_agent_worker(line));
        assert!(!is_agent_worker("396140 agent --resume=abc-def"));
    }

    #[test]
    fn command_line_targets_session_matches_resume() {
        let uuid = "d77a4a3f-145d-4350-856f-5ad28eed930d";
        let line = format!("396140 agent --resume={uuid}");
        assert!(command_line_targets_session(&line, uuid));
        assert!(!command_line_targets_session(&line, "00000000-0000-0000-0000-000000000000"));
    }

    #[test]
    fn parse_pid_reads_leading_number() {
        assert_eq!(parse_pid("12345 /usr/bin/agent --foo"), Some(12345));
        assert_eq!(parse_pid("not-a-pid"), None);
    }

    #[test]
    fn l3_lock_skip_reason_formats() {
        let reason = l3_lock_skip_reason(&["1 Cursor --type=zygote".into()]);
        assert!(reason.starts_with("l3_locked(1 proc"));
    }
}
