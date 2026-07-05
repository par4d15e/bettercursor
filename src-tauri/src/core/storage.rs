//! bettercursor storage layer — WAL-safe SQLite read for Cursor databases.
//!
//! Ported from `bettercursor/storage.py` (Python reference, 254 lines).
//!
//! Reads: copy the DB + WAL/SHM to a temp file, checkpoint WAL, query the copy.
//!        Avoids lock contention with a running Cursor.
//!
//! Supports both:
//!   - state.vscdb (Layer 3, Electron) — `ItemTable` and `cursorDiskKV` tables
//!   - store.db    (Layer 2, CLI)      — `blobs` and `meta` tables
//!
//! v0.1 is **read-only** for the `CursorRead` path. `open_write_staging_copy`
//! supports the L3 inject write path in `core::sync::write_layer3`.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Open a database in WAL-safe read mode by copying it to a temp directory.
///
/// The returned `Connection` is the read-only handle; the `TempDir` must be
/// kept alive (drop it explicitly via `_guard` field on the returned struct).
pub fn open_read(db_path: impl AsRef<Path>) -> Result<CursorRead> {
    let db_path = db_path.as_ref();
    if !db_path.exists() {
        anyhow::bail!("DB not found: {}", db_path.display());
    }

    let tmp = TempDir::new().context("create temp dir for read copy")?;
    let tmp_db = tmp.path().join(db_path.file_name().unwrap());

    std::fs::copy(db_path, &tmp_db)
        .with_context(|| format!("copy {} → {}", db_path.display(), tmp_db.display()))?;

    for suffix in ["-wal", "-shm"] {
        let sidecar = db_path.with_extension(format!(
            "{}{}",
            db_path.extension().and_then(|s| s.to_str()).unwrap_or("vscdb"),
            suffix
        ));
        if sidecar.exists() {
            let dst = tmp.path().join(format!(
                "{}{}",
                tmp_db.file_name().and_then(|s| s.to_str()).unwrap_or("db"),
                suffix
            ));
            let _ = std::fs::copy(&sidecar, &dst);
        }
    }

    let conn = Connection::open(&tmp_db).context("open temp DB")?;
    // Best-effort WAL checkpoint; ignore "not in WAL mode" errors.
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");

    Ok(CursorRead {
        conn,
        _tmp: tmp,
        source: db_path.to_path_buf(),
    })
}

/// A read-only handle to a copied Cursor SQLite database.
pub struct CursorRead {
    conn: Connection,
    _tmp: TempDir,
    source: PathBuf,
}

impl CursorRead {
    /// Get a string value by key from the named table (default: `ItemTable`).
    pub fn get_item(&self, key: &str, table: &str) -> Result<Option<String>> {
        let sql = format!("SELECT value FROM {table} WHERE key = ?");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([key])?;
        if let Some(row) = rows.next()? {
            let v: rusqlite::types::Value = row.get(0)?;
            Ok(Some(value_to_string(v)))
        } else {
            Ok(None)
        }
    }

    /// Get a binary blob by key from the named table.
    pub fn get_item_binary(&self, key: &str, table: &str) -> Result<Option<Vec<u8>>> {
        let sql = format!("SELECT value FROM {table} WHERE key = ?");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([key])?;
        if let Some(row) = rows.next()? {
            let v: rusqlite::types::Value = row.get(0)?;
            Ok(Some(value_to_bytes(v)))
        } else {
            Ok(None)
        }
    }

    /// Get a JSON-decoded value by key from the named table.
    pub fn get_json(&self, key: &str, table: &str) -> Result<Option<serde_json::Value>> {
        match self.get_item(key, table)? {
            Some(s) => Ok(serde_json::from_str(&s).ok()),
            None => Ok(None),
        }
    }

    /// Decode a `store.db` `meta` row — values are hex-encoded JSON in
    /// cursor-agent builds, occasionally raw JSON in older snapshots.
    pub fn get_store_meta_json(&self, key: &str) -> Result<Option<serde_json::Value>> {
        match self.get_item_binary(key, "meta")? {
            Some(bytes) => Ok(decode_store_meta_value(&bytes)),
            None => Ok(None),
        }
    }

    /// List all keys in the named table, optionally filtered by a SQL LIKE prefix.
    pub fn list_keys(&self, prefix: &str, table: &str) -> Result<Vec<String>> {
        let sql = if prefix.is_empty() {
            format!("SELECT key FROM {table}")
        } else {
            format!("SELECT key FROM {table} WHERE key LIKE ?")
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = if prefix.is_empty() {
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map([format!("{prefix}%")], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    }

    /// List keys whose name contains `substr` (SQL `LIKE %substr%`).
    pub fn list_keys_containing(&self, substr: &str, table: &str) -> Result<Vec<String>> {
        let sql = format!("SELECT key FROM {table} WHERE key LIKE ?");
        let mut stmt = self.conn.prepare(&sql)?;
        let pattern = format!("%{substr}%");
        let rows = stmt
            .query_map([pattern], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// List all keys in the `blobs` table (Layer 2).
    pub fn list_blob_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM blobs")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Path of the original database this was copied from.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Run `PRAGMA integrity_check` on the WAL-merged read copy.
    pub fn integrity_check(&self) -> Result<()> {
        let ok: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if ok != "ok" {
            anyhow::bail!("integrity_check failed: {ok}");
        }
        Ok(())
    }
}

/// One-shot helper: open a fresh read handle and return the JSON-decoded value.
pub fn read_json(db_path: impl AsRef<Path>, key: &str, table: &str) -> Result<Option<serde_json::Value>> {
    let r = open_read(db_path)?;
    r.get_json(key, table)
}

/// One-shot helper: list keys with prefix.
pub fn read_keys(db_path: impl AsRef<Path>, prefix: &str, table: &str) -> Result<Vec<String>> {
    let r = open_read(db_path)?;
    r.list_keys(prefix, table)
}

/// Fail fast when the live DB is corrupt (via WAL-merged read copy).
pub fn assert_db_integrity(db_path: impl AsRef<Path>) -> Result<()> {
    let r = open_read(db_path)?;
    r.integrity_check()
}

/// Sidecar path: `state.vscdb` + `-wal` / `-shm`.
pub fn sidecar_path(base: &Path, suffix: &str) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Remove WAL/SHM after replacing the main DB file.
pub fn remove_wal_sidecars(db_path: &Path) {
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(sidecar_path(db_path, suffix));
    }
}

/// Copy `src` → `dest`, checkpoint WAL into main on the copy, verify integrity.
/// Returns a writable connection — caller must `drop` before `atomic_replace`.
pub fn open_write_staging_copy(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<Connection> {
    let src = src.as_ref();
    let dest = dest.as_ref();
    std::fs::copy(src, dest).with_context(|| {
        format!(
            "copy {} → {}",
            src.display(),
            dest.display()
        )
    })?;
    for suffix in ["-wal", "-shm"] {
        let side = sidecar_path(src, suffix);
        if side.exists() {
            std::fs::copy(&side, sidecar_path(dest, suffix))?;
        }
    }
    let conn = Connection::open(dest).context("open staging state.vscdb")?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .context("wal_checkpoint(TRUNCATE) on staging copy")?;
    remove_wal_sidecars(dest);
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if ok != "ok" {
        anyhow::bail!("staging copy integrity_check failed: {ok}");
    }
    Ok(conn)
}

// ── Helpers ───────────────────────────────────────────────────

/// Decode `store.db` `meta` cell bytes — hex-encoded JSON (cursor-agent)
/// or raw JSON in older snapshots.
pub fn decode_store_meta_value(raw: &[u8]) -> Option<serde_json::Value> {
    if raw.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_slice(raw) {
        return Some(v);
    }
    let text = String::from_utf8_lossy(raw);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(bytes) = hex::decode(trimmed) {
        if let Ok(v) = serde_json::from_slice(&bytes) {
            return Some(v);
        }
    }
    None
}

fn value_to_string(v: rusqlite::types::Value) -> String {
    use rusqlite::types::Value::*;
    match v {
        Text(s) => s,
        Blob(b) => String::from_utf8_lossy(&b).into_owned(),
        Integer(i) => i.to_string(),
        Real(f) => f.to_string(),
        Null => String::new(),
    }
}

fn value_to_bytes(v: rusqlite::types::Value) -> Vec<u8> {
    use rusqlite::types::Value::*;
    match v {
        Blob(b) => b,
        Text(s) => s.into_bytes(),
        Integer(i) => i.to_le_bytes().to_vec(),
        Real(f) => f.to_le_bytes().to_vec(),
        Null => Vec::new(),
    }
}

// Silence unused warning for OpenFlags (kept for future write API).
#[allow(dead_code)]
const _OPEN_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_ONLY;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_store_meta_value_parses_hex_json() {
        let meta = serde_json::json!({
            "agentId": "b766a3e8-e1ff-4ba8-9b7a-39873aceb6ee",
            "name": "New Agent",
            "subagentInfo": {
                "parentAgentId": "33de2d97-940e-4335-a4ab-1f1a5b63243c",
                "rootParentAgentId": "33de2d97-940e-4335-a4ab-1f1a5b63243c",
                "typeName": "generalPurpose"
            }
        });
        let hex = hex::encode(serde_json::to_string(&meta).unwrap().as_bytes());
        let v = decode_store_meta_value(hex.as_bytes()).expect("hex meta");
        assert!(v.get("subagentInfo").is_some());
    }

    #[test]
    fn read_missing_db_errors() {
        let r = open_read("/nonexistent/path/state.vscdb");
        assert!(r.is_err());
    }
}
