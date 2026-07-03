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
//! v0.1 is **read-only**. Write functions (Phase T3 / delete) are not yet ported.

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

// ── Helpers ───────────────────────────────────────────────────

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
    fn read_missing_db_errors() {
        let r = open_read("/nonexistent/path/state.vscdb");
        assert!(r.is_err());
    }
}
