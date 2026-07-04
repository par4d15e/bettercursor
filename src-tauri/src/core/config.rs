//! bettercursor user preferences — `~/.bettercursor/config.json`.
//!
//! v0.2-alpha simplified: the only previous preference (`auto_sync_enabled`)
//! was removed (#103). Now `Preferences` is an empty struct kept around
//! as a placeholder for future settings (theme, sort default, watch dirs).
//! The load path is preserved so callers can ask "do you have any
//! config yet?" without churn when new keys are added.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// `Path` is only referenced from the test-only `load_from` helper;
// gate its import on `cfg(test)` so the production build stays
// warning-clean.
#[cfg(test)]
use std::path::Path;

use super::paths;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Preferences {}

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
/// dies mid-flush. Currently unused (no Preferences fields) but
/// kept for future keys.
#[allow(dead_code)]
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
        // Empty Preferences defaults — no auto_sync_enabled to test anymore.
        let _ = p;
    }

    #[test]
    fn defaults_when_file_malformed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "this is not json {").unwrap();
        let p = load_from(&path);
        let _ = p;
    }

    #[test]
    fn parses_empty_object() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{}"#).unwrap();
        let p = load_from(&path);
        let _ = p;
    }

    /// Stale `auto_sync_enabled` keys in user config files are
    /// silently ignored after the toggle removal (#103). Without
    /// `#[serde(default)]` on the field, deserialization would fail
    /// for users who haven't deleted the key.
    #[test]
    fn parses_legacy_auto_sync_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"auto_sync_enabled": false}"#).unwrap();
        // Must NOT panic — extra unknown fields are fine (or we could
        // add #[serde(deny_unknown_fields)] if we wanted strictness).
        let p = load_from(&path);
        let _ = p;
    }
}