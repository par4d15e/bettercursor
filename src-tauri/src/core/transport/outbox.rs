//! 离线 outbox — `~/.bettercursor/outbox/<peer_id>/` JSON 队列 (SYNC_DESIGN §5.2).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn outbox_dir() -> PathBuf {
    crate::core::paths::bettercursor_dir().join("outbox")
}

pub fn peer_outbox_dir(peer_id: &str) -> PathBuf {
    outbox_dir().join(peer_id)
}

pub fn processed_dir(peer_id: &str) -> PathBuf {
    peer_outbox_dir(peer_id).join(".processed")
}

/// Enqueue a v4 snapshot JSON for later flush when peer is offline / locked.
pub fn enqueue(peer_id: &str, uuid: &str, body: &str) -> Result<PathBuf> {
    let dir = peer_outbox_dir(peer_id);
    std::fs::create_dir_all(&dir)?;
    let ts = chrono::Utc::now().timestamp_millis();
    let path = dir.join(format!("{uuid}-{ts}.json"));
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path).with_context(|| format!("atomic rename {}", path.display()))?;
    Ok(path)
}

pub fn list_pending(peer_id: &str) -> Result<Vec<PathBuf>> {
    let dir = peer_outbox_dir(peer_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    files.sort();
    Ok(files)
}

pub fn mark_processed(peer_id: &str, path: &Path) -> Result<()> {
    let dest_dir = processed_dir(peer_id);
    std::fs::create_dir_all(&dest_dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("outbox path has no filename"))?;
    let dest = dest_dir.join(name);
    std::fs::rename(path, &dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _lock = crate::core::paths::test_home_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("BETTERCURSOR_TEST_HOME", tmp.path());
        f();
        std::env::remove_var("BETTERCURSOR_TEST_HOME");
    }

    #[test]
    fn enqueue_and_list_round_trip() {
        with_temp_home(|| {
            let p = enqueue("peer-a", "uuid-1", r#"{"version":4}"#).unwrap();
            assert!(p.exists());
            let pending = list_pending("peer-a").unwrap();
            assert_eq!(pending.len(), 1);
            mark_processed("peer-a", &pending[0]).unwrap();
            assert!(list_pending("peer-a").unwrap().is_empty());
        });
    }
}
