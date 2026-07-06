//! bettercursor path resolution — where Cursor stores its 4 layers.
//!
//! Ported from `bettercursor/paths.py` (Python reference, 182 lines).
//! See PRD §4.2 for the 4-layer storage model.
//!
//! Platform-aware:
//!   - macOS: `~/Library/Application Support/Cursor/User/`
//!   - Linux: `~/.config/Cursor/User/`
//!   - Layer 1 (JSONL): `~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl`
//!   - Layer 2 (CLI):   `~/.cursor/chats/<md5(cwd)>/<uuid>/store.db`
//!   - Layer 3 (Electron): `<user_dir>/globalStorage/state.vscdb`
//!
//! Session UUID (see SYNC_DESIGN §2.5 Q6):
//!   - Valid CLI session: L1 directory name == L2 directory name == composer_id.
//!   - Valid Desktop session: L3 `composerData:<uuid>` (+ `bubbleId:<uuid>:*`); L2 absent.
//!   - L2 session pool vs L3 session pool are usually disjoint on mixed-use machines;
//!     that is stack-vs-stack, not L1-vs-L2 mismatch.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

fn effective_home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(p) = std::env::var_os("BETTERCURSOR_TEST_HOME") {
        return Some(PathBuf::from(p));
    }
    home::home_dir()
}

#[cfg(test)]
pub fn test_home_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Return the Cursor User data directory for the current platform.
pub fn cursor_user_dir() -> Result<PathBuf> {
    let home = effective_home_dir().context("could not determine home directory")?;
    let p = match std::env::consts::OS {
        "macos" => home
            .join("Library")
            .join("Application Support")
            .join("Cursor")
            .join("User"),
        "linux" => home.join(".config").join("Cursor").join("User"),
        "windows" => home
            .join("AppData")
            .join("Roaming")
            .join("Cursor")
            .join("User"),
        other => anyhow::bail!("unsupported platform: {other}"),
    };
    Ok(p)
}

/// Path to Layer 3 SQLite: `<user_dir>/globalStorage/state.vscdb`.
pub fn global_db_path() -> Result<PathBuf> {
    Ok(cursor_user_dir()?.join("globalStorage").join("state.vscdb"))
}

/// Path to Layer 3 workspace storage dir (one state.vscdb per workspace).
pub fn workspace_storage_dir() -> Result<PathBuf> {
    Ok(cursor_user_dir()?.join("workspaceStorage"))
}

/// Path to a specific workspace's state.vscdb (Layer 3 per-workspace).
pub fn workspace_db(workspace_hash: &str) -> Result<PathBuf> {
    Ok(workspace_storage_dir()?
        .join(workspace_hash)
        .join("state.vscdb"))
}

/// `~/.cursor/projects/` — parent of all Layer 1 (JSONL) directories.
pub fn cursor_projects_dir() -> PathBuf {
    effective_home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cursor")
        .join("projects")
}

/// `~/.cursor/chats/` — parent of all `<md5(cwd)>/<uuid>/store.db` directories.
pub fn chats_dir() -> PathBuf {
    effective_home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cursor")
        .join("chats")
}

/// MD5 hex of `cwd`; identifies a project's Layer 2 root.
///
/// Note: we use MD5 (not SHA256) to match cursaves' layout for cross-compat
/// with snapshots produced by the Python reference implementation.
pub fn chat_root_for(cwd: impl AsRef<Path>) -> String {
    let path_str = cwd.as_ref().to_string_lossy().into_owned();
    format!("{:x}", md5::compute(path_str.as_bytes()))
}

/// Layer 2 path for a specific session: `~/.cursor/chats/<md5>/<uuid>/`.
pub fn chat_dir_for(cwd: impl AsRef<Path>, composer_id: &str) -> PathBuf {
    chats_dir().join(chat_root_for(cwd)).join(composer_id)
}

/// The `store.db` file inside a chat_dir.
pub fn store_db_for(cwd: impl AsRef<Path>, composer_id: &str) -> PathBuf {
    chat_dir_for(cwd, composer_id).join("store.db")
}

/// Find `~/.cursor/chats/<md5>/<uuid>/store.db` when `cwd` is unknown.
pub fn find_store_db_for_uuid(uuid: &str) -> Option<PathBuf> {
    let chats = chats_dir();
    if !chats.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&chats).ok()?;
    for entry in entries.flatten() {
        let db = entry.path().join(uuid).join("store.db");
        if db.is_file() {
            return Some(db);
        }
    }
    None
}

/// Resolve Layer 2 `store.db` — prefer `md5(cwd)` path, scan chats dir as fallback.
pub fn resolve_store_db_for(uuid: &str, cwd: &str) -> Option<PathBuf> {
    if !cwd.trim().is_empty() {
        let p = store_db_for(cwd, uuid);
        if p.is_file() {
            return Some(p);
        }
    }
    find_store_db_for_uuid(uuid)
}

/// `~/.bettercursor/` — bettercursor state directory. Currently
/// only `config.json` lives here (post-v0.2-alpha: no queue files,
/// no offline apply scripts — sync.rs writes inline).
pub fn bettercursor_dir() -> PathBuf {
    let p = effective_home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".bettercursor");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// `~/.bettercursor/config.json` — user preferences (auto-sync on/off, etc).
pub fn config_file() -> PathBuf {
    bettercursor_dir().join("config.json")
}

/// v0.3.0: `~/.bettercursor/unified.db` — read-cache + archive + sync_runs
/// index (per SYNC_DESIGN §3). Lives next to `transports.json` and
/// `config.json` so the whole bettercursor state sits in one dir that's
/// easy to back up / inspect with `sqlite3 ~/.bettercursor/unified.db`.
pub fn unified_db_path() -> PathBuf {
    bettercursor_dir().join("unified.db")
}

/// Convert `/Users/x/y` → `Users-x-y` (cursaves' format for Layer 1 path segment).
pub fn sanitize_project_path(project_path: &str) -> String {
    project_path
        .trim_matches('/')
        .replace('/', "-")
        .replace('\\', "-")
}

/// Locate the Layer 1 JSONL transcript for a given session uuid.
///
/// Cursor stores each session as either:
///   (a) `<chat_root>/agent-transcripts/<uuid>/<uuid>.jsonl`, or
///   (b) `<chat_root>/agent-transcripts/<uuid>.jsonl` (older layout).
///
/// Either is returned when present.
///
/// v0.2.2: unified from the previously-duplicated
/// `core::canonical::find_jsonl_for` and `core::inject::find_layer1_jsonl`
/// into a single canonical home in `paths`. Both old callers have been
/// migrated (`canonical::read_conversation` and
/// `inject::parse_layer1_bubbles`'s caller `core::sync::read_layer1`).
pub fn find_layer1_jsonl_for(uuid: &str) -> Option<PathBuf> {
    let projects = cursor_projects_dir();
    if !projects.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&projects).ok()?;
    for project in entries.flatten() {
        let transcripts = project.path().join("agent-transcripts");
        if !transcripts.is_dir() {
            continue;
        }
        let in_dir = transcripts.join(uuid).join(format!("{uuid}.jsonl"));
        if in_dir.is_file() {
            return Some(in_dir);
        }
        let flat = transcripts.join(format!("{uuid}.jsonl"));
        if flat.is_file() {
            return Some(flat);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_root_matches_python() {
        // The Python `chat_root_for("/home/eric/workspace/enenzuo")` produces
        // c19d07070edc77b1fdcdaf0dfecaf97f. We verify parity.
        let root = chat_root_for("/home/eric/workspace/enenzuo");
        assert_eq!(root, "c19d07070edc77b1fdcdaf0dfecaf97f");
    }

    #[test]
    fn sanitize_strips_slashes() {
        assert_eq!(sanitize_project_path("/Users/x/y"), "Users-x-y");
        assert_eq!(sanitize_project_path("a/b/c"), "a-b-c");
    }
}
