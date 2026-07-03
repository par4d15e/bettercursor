//! bettercursor user preferences — `~/.bettercursor/config.json`.
//!
//! Persisted across restarts. v0.2 currently stores only one key
//! (`auto_sync_enabled`), but the schema is forward-compatible: read
//! returns `Preferences::default()` if the file is missing or malformed,
//! write always serializes the full struct.
//!
//! Why JSON, not SQLite: at <10 keys and human-edited occasionally,
//! JSON keeps the file trivial to `cat`, diff, and back up. SQLite
//! would be overkill and adds a bundled dep we already avoid for
//! read-side storage.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    /// Whether the fs watcher thread should re-scan when its child
    /// directories change. Default false (user must opt in, matching
    /// ccswitch's local-route toggle).
    #[serde(default = "default_false")]
    pub auto_sync_enabled: bool,
}

const fn default_false() -> bool {
    false
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            auto_sync_enabled: false,
        }
    }
}

/// Read preferences from disk. Missing or unreadable file → defaults.
/// Malformed JSON → logged warn + defaults (don't break startup).
pub fn load() -> Preferences {
    let path = paths::config_file();
    if !path.exists() {
        return Preferences::default();
    }
    match std::fs::read_to_string(&path)
        .context("read config")
        .and_then(|s| serde_json::from_str::<Preferences>(&s).context("parse config"))
    {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "config at {} unreadable, falling back to defaults: {e:#}",
                path.display()
            );
            Preferences::default()
        }
    }
}

/// Persist preferences atomically: write to a temp file in the same
/// directory, then rename. Avoids half-written config if the process
/// dies mid-flush.
pub fn save(prefs: &Preferences) -> Result<()> {
    let path = paths::config_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("create config dir {}", parent.display())
        })?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(prefs).context("serialize config")?;
    std::fs::write(&tmp, body)
        .with_context(|| format!("write tmp config to {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename tmp → {}", path.display()))?;
    Ok(())
}

/// Update a single field and persist. Convenience for the
/// `set_auto_sync(enabled)` Tauri command path.
pub fn set_auto_sync(enabled: bool) -> Result<Preferences> {
    let mut prefs = load();
    prefs.auto_sync_enabled = enabled;
    save(&prefs)?;
    Ok(prefs)
}

/// Diagnostic: emit the path string for the runtime log.
pub fn config_path_display() -> String {
    paths::config_file().display().to_string()
}

/// Test-only helper: load prefs from an arbitrary path. Production
/// code uses `load()` above which reads from `paths::config_file()`.
#[cfg(test)]
fn load_from(path: &Path) -> Preferences {
    if !path.exists() {
        return Preferences::default();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Preferences>(&s).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        let p = load_from(&path);
        assert!(!p.auto_sync_enabled);
    }

    #[test]
    fn defaults_when_file_malformed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "this is not json {").unwrap();
        let p = load_from(&path);
        assert!(!p.auto_sync_enabled);
    }

    #[test]
    fn parses_explicit_false() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"auto_sync_enabled": false}"#).unwrap();
        let p = load_from(&path);
        assert!(!p.auto_sync_enabled);
    }

    #[test]
    fn parses_explicit_true() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"auto_sync_enabled": true}"#).unwrap();
        let p = load_from(&path);
        assert!(p.auto_sync_enabled);
    }
}
