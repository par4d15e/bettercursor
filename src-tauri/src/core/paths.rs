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

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Return the Cursor User data directory for the current platform.
pub fn cursor_user_dir() -> Result<PathBuf> {
    let home = home::home_dir().context("could not determine home directory")?;
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
    Ok(cursor_user_dir()?
        .join("globalStorage")
        .join("state.vscdb"))
}

/// Path to Layer 3 workspace storage dir (one state.vscdb per workspace).
pub fn workspace_storage_dir() -> Result<PathBuf> {
    Ok(cursor_user_dir()?.join("workspaceStorage"))
}

/// Path to a specific workspace's state.vscdb (Layer 3 per-workspace).
pub fn workspace_db(workspace_hash: &str) -> Result<PathBuf> {
    Ok(workspace_storage_dir()?.join(workspace_hash).join("state.vscdb"))
}

/// `~/.cursor/projects/` — parent of all Layer 1 (JSONL) directories.
pub fn cursor_projects_dir() -> PathBuf {
    home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cursor")
        .join("projects")
}

/// `~/.cursor/chats/` — parent of all `<md5(cwd)>/<uuid>/store.db` directories.
pub fn chats_dir() -> PathBuf {
    home::home_dir()
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

/// `~/.bettercursor/` — bettercursor state directory.
pub fn bettercursor_dir() -> PathBuf {
    let p = home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".bettercursor");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Convert `/Users/x/y` → `Users-x-y` (cursaves' format for Layer 1 path segment).
pub fn sanitize_project_path(project_path: &str) -> String {
    project_path
        .trim_matches('/')
        .replace('/', "-")
        .replace('\\', "-")
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
