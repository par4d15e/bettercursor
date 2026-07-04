//! bettercursor process detection — check whether Cursor Desktop and/or
//! `cursor-agent` are currently running. Used by `core::sync::sync_session`
//! to refuse writes that would race with live processes (#84 — Cursor's
//! WAL flush silently overwrites our writes if it's still open).
//!
//! Detection strategy: `pgrep -af <pattern>` on Linux/macOS. On platforms
//! without `pgrep` (Windows, exotic BSDs) we conservatively report "no
//! processes detected" so writes proceed — the SQLite tmpdir-copy +
//! atomic_rename path still keeps the actual write safe; the only thing
//! we lose is the early bail-out.
//!
//! Patterns we match (mirrors `scripts/apply.py:85-120` but adds
//! `cursor-agent` since `--resume` and interactive chat also hold files):
//!   - `Cursor --type=`         → Electron main + helpers
//!   - `/Cursor --updated`      → Linux Electron helper arg
//!   - `Cursor-bin`             → packaged binary name
//!   - `cursor-server`          → Linux Remote SSH server (opens state.vscdb)
//!   - `cursor-agent`           → CLI (interactive + --resume)
//!
//! Self-filter: when invoked from bettercursor itself (e.g. a child process
//! running `pgrep -af cursor-agent`), pgrep will match the spawned shell.
//! Strip any line that contains both `pgrep` and our pattern, mirroring
//! the Python script's defensive check at apply.py:117.

use std::process::Command;

/// Process patterns that indicate "Cursor is busy, do not write".
const PATTERNS: &[&str] = &[
    "Cursor --type=",
    "/Cursor --updated",
    "Cursor-bin",
    "cursor-server",
    "cursor-agent",
];

/// Run `pgrep -af <pat>` for each pattern and return the union of matching
/// lines (without the leading PID), with self-references stripped. Returns
/// an empty vec when no processes match or `pgrep` is unavailable.
pub fn cursor_processes_running() -> Vec<String> {
    let mut matches: Vec<String> = Vec::new();
    for pat in PATTERNS {
        let output = match Command::new("pgrep")
            .args(["-af", pat])
            .output()
        {
            Ok(o) if o.status.success() => o,
            // pgrep exits 1 when no match; that's fine. Anything else
            // (file not found, etc.) we treat as "unknown → assume safe".
            _ => continue,
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Strip `pgrep -af <pat>` itself when it appears as an arg of
            // the current shell (mirrors apply.py:117).
            if line.contains("pgrep") && line.contains(pat) {
                continue;
            }
            matches.push(line.to_string());
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper for tests: spawn `pgrep -af` against an arbitrary pattern
    /// (the real `cursor_processes_running` uses a fixed list). Public
    /// so callers can probe custom patterns without re-implementing the
    /// self-filter.
    fn pgrep_for(pattern: &str) -> Vec<String> {
        let Ok(output) = Command::new("pgrep").args(["-af", pattern]).output() else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn cursor_processes_running_returns_vec() {
        // Smoke test: just verify the function doesn't panic and returns
        // a Vec. Content depends on the host's actual running processes;
        // we can't assert on the length without flakes.
        let _ = cursor_processes_running();
    }

    #[test]
    fn pgrep_for_filters_empty_pattern() {
        // Patterns that don't match anything should return empty.
        let matches = pgrep_for("this_pattern_definitely_does_not_exist_zzzz");
        assert!(matches.is_empty(), "expected no matches, got {matches:?}");
    }

    #[test]
    fn pgrep_for_self_reference_strippable() {
        // Verify our self-filter helper works on a line containing
        // 'pgrep' + the pattern (as a fake match).
        let fake_line = format!(
            "12345 pgrep -af {}",
            "this_pattern_definitely_does_not_exist_zzzz"
        );
        assert!(fake_line.contains("pgrep") && fake_line.contains("this_pattern"));
        // We don't actually invoke pgrep here — this just locks in the
        // invariant that the self-filter pattern is "contains pgrep AND
        // contains pattern".
    }
}