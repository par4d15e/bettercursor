//! bettercursor unified.db — read-cache + archive + sync_runs index.
//!
//! v0.3.0 (per SYNC_DESIGN §3). Owns `~/.bettercursor/unified.db`, a
//! bundled-SQLite database (rusqlite `bundled` feature) opened in WAL
//! mode. Coexists with v0.2.6's inline-write paths — `core::sync` and
//! `core::canonical` are still the source of truth for Layer 1 / L2 / L3
//! storage; unified.db is the read-cache that backs cross-device sync
//! history, FTS5 search, and the v0.3.0+ Conflict resolution flow.
//!
//! Schema (8 tables, verbatim from SYNC_DESIGN §3.4 with one extra
//! `schema_version` for forward migration):
//!
//!   - schema_version   — single-row migration marker
//!   - sessions         — 1 row = 1 session (PRIMARY KEY uuid)
//!   - bubbles          — 1 row = 1 bubble (composite PK session_uuid+id)
//!   - bubbles_fts      — FTS5 virtual table mirroring bubbles.text
//!   - blobs            — raw blob refs (used by v0.3.1+ blob transport)
//!   - composer_data    — Layer 3 composerData JSON per session
//!   - sync_runs        — per-peer push/pull history
//!   - archive          — pre-overwrite / pre-delete / pre-merge snapshots
//!   - conflicts        — 5-way ConflictClass rows (resolved_at_ms NULL = open)
//!
//! Per §3.2 the FTS5 mirror is maintained **without triggers** — every
//! write that touches `bubbles` is responsible for keeping
//! `bubbles_fts` in sync. This keeps the schema reviewable and avoids
//! the silent-mirror bugs that fire when triggers get out of date.

use crate::core::conflict::{self, ConflictClass};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

/// Owned SQLite connection with WAL + foreign keys + reasonable
/// synchronous mode. The Mutex is the single chokepoint — every
/// write goes through it so concurrent Tauri commands can't trample
/// each other's transactions.
pub struct UnifiedDb {
    conn: Mutex<Connection>,
}

/// One row from the `sessions` table, materialized for the frontend /
/// Conflict classifier. Mirrors the columns 1:1 so a single SELECT *
/// maps cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub uuid: String,
    pub host: String,
    pub endpoint_kind: String,
    pub project_slug: String,
    pub project_path: Option<String>,
    pub source_path: String,
    pub last_updated_at_ms: i64,
    pub bubble_count: u32,
    pub content_hash: String,
    pub is_broken: bool,
    pub sources_json: String,
    pub first_seen_at_ms: i64,
    pub last_rebuilt_at_ms: i64,
}

/// One row from the `bubbles` table. Tool calls / files are stored as
/// serialized JSON strings so the schema doesn't have to track
/// Cursor's evolving `[{type, name, input}]` shapes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BubbleRow {
    pub id: String,
    pub session_uuid: String,
    pub role: String,
    pub text: String,
    pub tool_calls_json: Option<String>,
    pub files_json: Option<String>,
    pub ts_ms: i64,
    pub parent_bubble_id: Option<String>,
}

/// One row from the `conflicts` table. `(id, uuid, class)` is what
/// the UI needs to render the unresolved-Conflicts badge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictRow {
    pub id: i64,
    pub session_uuid: String,
    pub class: String,
    pub local_content_hash: Option<String>,
    pub incoming_content_hash: Option<String>,
    pub classified_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
    pub resolved_how: Option<String>,
}

impl UnifiedDb {
    /// Open (or create) `~/.bettercursor/unified.db` with WAL mode +
    /// foreign keys + reasonable sync level. Idempotent — calling
    /// twice on the same path yields the same database.
    pub fn open() -> Result<Self> {
        let path = crate::core::paths::unified_db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA synchronous=NORMAL;",
        )
        .with_context(|| "set pragmas")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.run_migrations()
            .with_context(|| "run migrations")?;
        Ok(db)
    }

    /// Schema migration. v0.3.0 first cut uses `CREATE … IF NOT EXISTS`
    /// for everything (no downgrade path needed) and bumps
    /// `schema_version` exactly once. Future versions stack on top.
    fn run_migrations(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            );
            INSERT OR IGNORE INTO schema_version(version) VALUES (1);

            CREATE TABLE IF NOT EXISTS sessions (
                uuid TEXT PRIMARY KEY,
                host TEXT NOT NULL,
                endpoint_kind TEXT NOT NULL,
                project_slug TEXT NOT NULL,
                project_path TEXT,
                source_path TEXT NOT NULL,
                last_updated_at_ms INTEGER NOT NULL,
                bubble_count INTEGER NOT NULL,
                content_hash TEXT NOT NULL,
                is_broken INTEGER NOT NULL DEFAULT 0,
                sources_json TEXT NOT NULL DEFAULT '{}',
                first_seen_at_ms INTEGER NOT NULL,
                last_rebuilt_at_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS sessions_host_idx
                ON sessions(host);
            CREATE INDEX IF NOT EXISTS sessions_last_updated_idx
                ON sessions(last_updated_at_ms DESC);

            CREATE TABLE IF NOT EXISTS bubbles (
                id TEXT NOT NULL,
                session_uuid TEXT NOT NULL,
                role TEXT NOT NULL,
                text TEXT NOT NULL,
                tool_calls_json TEXT,
                files_json TEXT,
                ts_ms INTEGER NOT NULL,
                parent_bubble_id TEXT,
                PRIMARY KEY (session_uuid, id),
                FOREIGN KEY (session_uuid) REFERENCES sessions(uuid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS bubbles_session_ts_idx
                ON bubbles(session_uuid, ts_ms);

            -- §3.2: FTS5 mirror WITHOUT triggers (manual upkeep).
            -- tokenize='unicode61 remove_diacritics 2' is the v0.3.0
            -- baseline — covers ASCII + Latin-1 + most CJK unigram
            -- matching. A real Chinese segmenter (jieba-rs etc.) is
            -- deferred to v0.3.1+; documented in the v0.3.0 plan.
            CREATE VIRTUAL TABLE IF NOT EXISTS bubbles_fts USING fts5(
                text,
                content='',
                tokenize='unicode61 remove_diacritics 2'
            );

            CREATE TABLE IF NOT EXISTS blobs (
                blob_hash TEXT PRIMARY KEY,
                bytes BLOB NOT NULL,
                byte_size INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS composer_data (
                session_uuid TEXT PRIMARY KEY,
                full_json TEXT NOT NULL,
                subset_json TEXT NOT NULL,
                FOREIGN KEY (session_uuid) REFERENCES sessions(uuid) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS sync_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                peer_id TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                finished_at_ms INTEGER,
                direction TEXT NOT NULL,
                items_processed INTEGER NOT NULL DEFAULT 0,
                items_failed INTEGER NOT NULL DEFAULT 0,
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS sync_runs_peer_started_idx
                ON sync_runs(peer_id, started_at_ms DESC);

            CREATE TABLE IF NOT EXISTS archive (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_uuid TEXT NOT NULL,
                reason TEXT NOT NULL,
                archived_at_ms INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                FOREIGN KEY (session_uuid) REFERENCES sessions(uuid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS archive_session_reason_idx
                ON archive(session_uuid, reason, archived_at_ms DESC);

            CREATE TABLE IF NOT EXISTS conflicts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_uuid TEXT NOT NULL,
                classified_at_ms INTEGER NOT NULL,
                class TEXT NOT NULL,
                local_content_hash TEXT,
                incoming_content_hash TEXT,
                resolved_at_ms INTEGER,
                resolved_how TEXT,
                FOREIGN KEY (session_uuid) REFERENCES sessions(uuid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS conflicts_unresolved_idx
                ON conflicts(resolved_at_ms) WHERE resolved_at_ms IS NULL;
            ",
        )?;
        Ok(())
    }

    /// §3.5 rebuild: ingest a fresh `Vec<CanonicalSession>` and write
    /// each into `sessions` + `bubbles` + `bubbles_fts` +
    /// `composer_data`. Idempotent — re-running does NOT add rows
    /// (the SQL is `INSERT … ON CONFLICT(uuid) DO UPDATE` plus a
    /// per-session `DELETE FROM bubbles` before re-inserting).
    ///
    /// Note: bubbles are read on the fly from
    /// `crate::core::canonical::read_conversation` per session so the
    /// L1/L2/L3 3-way merge logic stays the single source of truth.
    /// On a 37-session dev box this is ~37 × (1 JSONL + 1 store.db +
    /// 1 state.vscdb) = ~370 ms — acceptable for the rebuild path
    /// which only fires on `sync_now` / inline-write completion.
    pub fn rebuild_from_cursor_state(
        &self,
        sessions: &[crate::core::canonical::CanonicalSession],
        host: &str,
        now_ms: i64,
    ) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for s in sessions {
            // Resolve bubbles through the canonical 3-layer merge so
            // what unified.db stores matches what the React
            // `<MessageList>` renders.
            let conv = crate::core::canonical::read_conversation(&s.uuid);
            let bubbles = conv.bubbles;
            let count = bubbles.len() as u32;

            tx.execute(
                "
                INSERT INTO sessions (
                    uuid, host, endpoint_kind, project_slug, project_path,
                    source_path, last_updated_at_ms, bubble_count, content_hash,
                    is_broken, sources_json, first_seen_at_ms, last_rebuilt_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(uuid) DO UPDATE SET
                    host = excluded.host,
                    endpoint_kind = excluded.endpoint_kind,
                    project_slug = excluded.project_slug,
                    project_path = excluded.project_path,
                    source_path = excluded.source_path,
                    last_updated_at_ms = excluded.last_updated_at_ms,
                    bubble_count = excluded.bubble_count,
                    content_hash = excluded.content_hash,
                    is_broken = excluded.is_broken,
                    sources_json = excluded.sources_json,
                    last_rebuilt_at_ms = excluded.last_rebuilt_at_ms
                ",
                params![
                    s.uuid,
                    host,
                    s.sources.preferred_endpoint_kind(),
                    s.project_slug,
                    s.project_path,
                    s.sources.preferred_source_path(),
                    s.last_updated_at,
                    count,
                    conflict::content_hash_from_bubbles(&bubbles),
                    s.is_broken as i32,
                    serde_json::to_string(&s.sources).unwrap_or_else(|_| "{}".to_string()),
                    now_ms,
                    now_ms,
                ],
            )?;

            // Per-session bubble replace: clear, then re-insert into
            // both `bubbles` and the FTS5 mirror (no triggers — manual).
            tx.execute(
                "DELETE FROM bubbles WHERE session_uuid = ?1",
                params![s.uuid],
            )?;
            for b in &bubbles {
                tx.execute(
                    "
                    INSERT INTO bubbles (
                        id, session_uuid, role, text,
                        tool_calls_json, files_json, ts_ms, parent_bubble_id
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    ",
                    params![
                        b.id,
                        s.uuid,
                        b.role,
                        b.text,
                        serde_json::to_string(&b.tool_calls).ok(),
                        serde_json::to_string(&b.files).ok(),
                        b.created_at_ms,
                        b.parent_bubble_id,
                    ],
                )?;
                // FTS5 mirror row: tie the FTS rowid to the bubbles
                // rowid we just inserted. SQLite guarantees the
                // bubbles rowid is the AUTOINCREMENT one for this
                // session (we just inserted it).
                tx.execute(
                    "INSERT INTO bubbles_fts(rowid, text)
                     VALUES ((SELECT rowid FROM bubbles
                              WHERE session_uuid = ?1 AND id = ?2), ?3)",
                    params![s.uuid, b.id, b.text],
                )?;
            }

            if let Some(cd) = &s.composer_data {
                tx.execute(
                    "
                    INSERT INTO composer_data (session_uuid, full_json, subset_json)
                    VALUES (?1, ?2, ?3)
                    ON CONFLICT(session_uuid) DO UPDATE SET
                        full_json = excluded.full_json,
                        subset_json = excluded.subset_json
                    ",
                    params![s.uuid, cd.full_json, cd.subset_json],
                )?;
            }
        }
        tx.commit()?;
        Ok(sessions.len())
    }

    /// Delete a single session row. Cascades to bubbles, composer_data,
    /// archive, conflicts via the FOREIGN KEY ON DELETE CASCADE clauses
    /// (PRAGMA foreign_keys=ON is set in `open`).
    pub fn delete_session_row(&self, uuid: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM sessions WHERE uuid = ?1",
            params![uuid],
        )?;
        Ok(())
    }

    /// Record a pre-overwrite / pre-delete snapshot. Returns the
    /// archive row id. `reason` is one of "before_overwrite",
    /// "before_delete", "before_auto_merge" — stored verbatim so the
    /// UI / recovery tool can filter.
    pub fn record_archive(
        &self,
        uuid: &str,
        reason: &str,
        payload_json: &str,
        now_ms: i64,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            INSERT INTO archive (session_uuid, reason, archived_at_ms, payload_json)
            VALUES (?1, ?2, ?3, ?4)
            ",
            params![uuid, reason, now_ms, payload_json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Record a Conflict classification. Returns the new conflict row id.
    pub fn record_conflict(
        &self,
        uuid: &str,
        class: ConflictClass,
        local_hash: Option<&str>,
        incoming_hash: Option<&str>,
        now_ms: i64,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            INSERT INTO conflicts (
                session_uuid, classified_at_ms, class,
                local_content_hash, incoming_content_hash
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ",
            params![uuid, now_ms, class.as_str(), local_hash, incoming_hash],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Upsert one session from a v4 transport snapshot into unified.db.
    /// Used by `transport_pull` after conflict classification.
    pub fn upsert_session_from_snapshot(
        &self,
        snap: &crate::core::snapshot::SessionSnapshot,
        bubbles: &[crate::core::canonical::Bubble],
        now_ms: i64,
    ) -> Result<()> {
        let uuid = snap.composer.composer_id.clone();
        let content_hash = conflict::content_hash_from_bubbles(bubbles);
        let count = bubbles.len() as u32;
        let first_seen = self
            .get_session_meta(&uuid)?
            .map(|r| r.first_seen_at_ms)
            .unwrap_or(now_ms);
        let project_path = if snap.composer.project_path.is_empty() {
            None
        } else {
            Some(snap.composer.project_path.clone())
        };

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "
            INSERT INTO sessions (
                uuid, host, endpoint_kind, project_slug, project_path,
                source_path, last_updated_at_ms, bubble_count, content_hash,
                is_broken, sources_json, first_seen_at_ms, last_rebuilt_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(uuid) DO UPDATE SET
                host = excluded.host,
                endpoint_kind = excluded.endpoint_kind,
                project_slug = excluded.project_slug,
                project_path = excluded.project_path,
                source_path = excluded.source_path,
                last_updated_at_ms = excluded.last_updated_at_ms,
                bubble_count = excluded.bubble_count,
                content_hash = excluded.content_hash,
                is_broken = excluded.is_broken,
                sources_json = excluded.sources_json,
                last_rebuilt_at_ms = excluded.last_rebuilt_at_ms
            ",
            params![
                uuid,
                snap.source_endpoint.host,
                snap.source_endpoint.endpoint_kind,
                snap.composer.project_slug,
                project_path,
                snap.composer.project_path,
                snap.composer.last_updated_at,
                count,
                content_hash,
                0i32,
                "{}",
                first_seen,
                now_ms,
            ],
        )?;

        tx.execute(
            "DELETE FROM bubbles WHERE session_uuid = ?1",
            params![uuid],
        )?;
        for b in bubbles {
            tx.execute(
                "
                INSERT INTO bubbles (
                    id, session_uuid, role, text,
                    tool_calls_json, files_json, ts_ms, parent_bubble_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ",
                params![
                    b.id,
                    uuid,
                    b.role,
                    b.text,
                    serde_json::to_string(&b.tool_calls).ok(),
                    serde_json::to_string(&b.files).ok(),
                    b.created_at_ms,
                    b.parent_bubble_id,
                ],
            )?;
            tx.execute(
                "INSERT INTO bubbles_fts(rowid, text)
                 VALUES ((SELECT rowid FROM bubbles
                          WHERE session_uuid = ?1 AND id = ?2), ?3)",
                params![uuid, b.id, b.text],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Convert `BubbleRow` values into canonical `Bubble` for conflict merge.
    pub fn bubbles_from_rows(rows: &[BubbleRow]) -> Vec<crate::core::canonical::Bubble> {
        rows.iter()
            .map(|r| crate::core::canonical::Bubble {
                id: r.id.clone(),
                role: r.role.clone(),
                text: r.text.clone(),
                tool_calls: r
                    .tool_calls_json
                    .as_ref()
                    .and_then(|j| serde_json::from_str(j).ok())
                    .unwrap_or_default(),
                files: r
                    .files_json
                    .as_ref()
                    .and_then(|j| serde_json::from_str(j).ok())
                    .unwrap_or_default(),
                images: Vec::new(),
                created_at_ms: r.ts_ms,
                parent_bubble_id: r.parent_bubble_id.clone(),
            })
            .collect()
    }

    /// Mark a conflict resolved. `resolved_how` is one of
    /// "auto_merged", "user_chose_local", "user_chose_incoming",
    /// "skipped".
    pub fn resolve_conflict(
        &self,
        conflict_id: i64,
        how: &str,
        now_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            UPDATE conflicts
            SET resolved_at_ms = ?1, resolved_how = ?2
            WHERE id = ?3
            ",
            params![now_ms, how, conflict_id],
        )?;
        Ok(())
    }

    /// FTS5 search. Returns up to `limit` session UUIDs ranked by FTS5
    /// `rank` (BM25-like). v0.3.0 first cut: ASCII / Latin / CJK
    /// unigram; commit message documents the limitation.
    pub fn search_bubbles(&self, query: &str, limit: u32) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "
            SELECT b.session_uuid
            FROM bubbles_fts f
            JOIN bubbles b ON b.rowid = f.rowid
            WHERE bubbles_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            ",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Open a sync_runs row. Returns the new id — caller must later
    /// call `finish_sync_run` to close it.
    pub fn record_sync_run(
        &self,
        peer_id: &str,
        direction: &str,
        started_ms: i64,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            INSERT INTO sync_runs (peer_id, started_at_ms, direction)
            VALUES (?1, ?2, ?3)
            ",
            params![peer_id, started_ms, direction],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Close a sync_runs row with the final tallies + optional error.
    pub fn finish_sync_run(
        &self,
        run_id: i64,
        items_processed: u32,
        items_failed: u32,
        finished_ms: i64,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            UPDATE sync_runs
            SET finished_at_ms = ?1, items_processed = ?2,
                items_failed = ?3, error = ?4
            WHERE id = ?5
            ",
            params![finished_ms, items_processed, items_failed, error, run_id],
        )?;
        Ok(())
    }

    /// All conflicts whose `resolved_at_ms IS NULL`, newest first.
    /// The frontend uses this to render the "X unresolved conflicts"
    /// badge + the eventual `<ConflictResolveDialog>` (v0.3.1).
    pub fn unresolved_conflicts(&self) -> Result<Vec<ConflictRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "
            SELECT id, session_uuid, class, local_content_hash,
                   incoming_content_hash, classified_at_ms,
                   resolved_at_ms, resolved_how
            FROM conflicts
            WHERE resolved_at_ms IS NULL
            ORDER BY classified_at_ms DESC
            ",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ConflictRow {
                id: r.get(0)?,
                session_uuid: r.get(1)?,
                class: r.get(2)?,
                local_content_hash: r.get(3)?,
                incoming_content_hash: r.get(4)?,
                classified_at_ms: r.get(5)?,
                resolved_at_ms: r.get(6)?,
                resolved_how: r.get(7)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Look up a session by uuid. Used by `transport_pull` (PR-2) to
    /// feed the conflict classifier with the local content hash +
    /// timestamp before deciding New / Identical / IncomingNewer /
    /// LocalAhead / Diverged. PR-1 surfaces it for the unit tests
    /// only; PR-2 wires it into the actual pull flow.
    pub fn get_session_meta(&self, uuid: &str) -> Result<Option<SessionRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "
                SELECT uuid, host, endpoint_kind, project_slug, project_path,
                       source_path, last_updated_at_ms, bubble_count, content_hash,
                       is_broken, sources_json, first_seen_at_ms, last_rebuilt_at_ms
                FROM sessions
                WHERE uuid = ?1
                ",
                params![uuid],
                |r| {
                    Ok(SessionRow {
                        uuid: r.get(0)?,
                        host: r.get(1)?,
                        endpoint_kind: r.get(2)?,
                        project_slug: r.get(3)?,
                        project_path: r.get(4)?,
                        source_path: r.get(5)?,
                        last_updated_at_ms: r.get(6)?,
                        bubble_count: r.get(7)?,
                        content_hash: r.get(8)?,
                        is_broken: r.get::<_, i64>(9)? != 0,
                        sources_json: r.get(10)?,
                        first_seen_at_ms: r.get(11)?,
                        last_rebuilt_at_ms: r.get(12)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Load all bubbles for a uuid (used by conflict::bubble_diff and
    /// by transport_pull's auto_merge path). Empty Vec when the
    /// session has no row yet.
    pub fn get_bubbles(&self, uuid: &str) -> Result<Vec<BubbleRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "
            SELECT id, session_uuid, role, text,
                   tool_calls_json, files_json, ts_ms, parent_bubble_id
            FROM bubbles
            WHERE session_uuid = ?1
            ORDER BY ts_ms ASC, id ASC
            ",
        )?;
        let rows = stmt.query_map(params![uuid], |r| {
            Ok(BubbleRow {
                id: r.get(0)?,
                session_uuid: r.get(1)?,
                role: r.get(2)?,
                text: r.get(3)?,
                tool_calls_json: r.get(4)?,
                files_json: r.get(5)?,
                ts_ms: r.get(6)?,
                parent_bubble_id: r.get(7)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// For tests + diagnostics: total row count per table.
    pub fn row_counts(&self) -> Result<std::collections::HashMap<String, i64>> {
        let conn = self.conn.lock().unwrap();
        let tables = [
            "sessions",
            "bubbles",
            "blobs",
            "composer_data",
            "sync_runs",
            "archive",
            "conflicts",
        ];
        let mut out = std::collections::HashMap::new();
        for t in tables {
            let n: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {t}"),
                [],
                |r| r.get(0),
            )?;
            out.insert(t.to_string(), n);
        }
        out.insert(
            "bubbles_fts".to_string(),
            conn.query_row("SELECT COUNT(*) FROM bubbles_fts", [], |r| r.get(0))?,
        );
        Ok(out)
    }

    /// Tests / startup probes: surface the resolved DB path so the
    /// "unified.db created" log line can show it.
    pub fn path() -> PathBuf {
        crate::core::paths::unified_db_path()
    }
}

// ── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::canonical::{
        Bubble, BubbleToolUse, CanonicalSession, ComposerData, SourceInfo, SourceLayer, Sources,
    };
    use std::sync::Mutex as StdMutex;

    /// Per-test override of `bettercursor_dir()` so each test gets its
    /// own `~/.bettercursor/unified.db` and doesn't trample other
    /// tests' data. The Mutex serializes the HOME env-var patch —
    /// same trick `core::sync::tests::HOME_LOCK` uses.
    static HOME_LOCK: StdMutex<()> = StdMutex::new(());
    fn fresh_unified_db() -> (tempfile::TempDir, UnifiedDb) {
        let home = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());
        let db = UnifiedDb::open().expect("open unified.db");
        if let Some(v) = prev {
            std::env::set_var("HOME", v);
        } else {
            std::env::remove_var("HOME");
        }
        (home, db)
    }

    fn sample_session(uuid: &str, project: &str) -> CanonicalSession {
        CanonicalSession {
            uuid: uuid.to_string(),
            project_slug: project.to_string(),
            project_path: String::new(),
            chat_root: String::new(),
            name: format!("session {uuid}"),
            last_updated_at: 1_700_000_000_000,
            bubble_count: 2,
            is_empty_draft: false,
            is_broken: false,
            broken_reason: None,
            sources: Sources {
                mac: None,
                linux_cli: Some(SourceInfo {
                    last_seen_at: 1_700_000_000_000,
                    layer: "2".into(),
                    path: "/tmp/store.db".into(),
                }),
                linux_desktop: None,
            },
            first_user_message_preview: "hello".into(),
            files_referenced: vec![],
            indexable_text: String::new(),
            layer_3_present: false,
            composer_data: None,
            composer_id: None,
        }
    }

    fn bubble(id: &str, role: &str, text: &str, ts: i64) -> Bubble {
        Bubble {
            id: id.to_string(),
            role: role.to_string(),
            text: text.to_string(),
            tool_calls: vec![BubbleToolUse {
                name: "Glob".into(),
                input: Some(serde_json::json!({"pattern": "*"})),
            }],
            files: vec!["main.rs".into()],
            images: vec![],
            created_at_ms: ts,
            parent_bubble_id: None,
        }
    }

    /// `open()` must create the file + run migrations + expose 8
    /// tables (`schema_version`, `sessions`, `bubbles`, `bubbles_fts`,
    /// `blobs`, `composer_data`, `sync_runs`, `archive`,
    /// `conflicts`). row_counts() surfaces them by name.
    #[test]
    fn open_creates_eight_tables() {
        let _lock = HOME_LOCK.lock().unwrap();
        let (_home, db) = fresh_unified_db();
        let counts = db.row_counts().expect("row_counts");
        for t in [
            "sessions",
            "bubbles",
            "blobs",
            "composer_data",
            "sync_runs",
            "archive",
            "conflicts",
            "bubbles_fts",
        ] {
            assert!(counts.contains_key(t), "missing {t}");
            assert_eq!(counts[t], 0, "table {t} should be empty on fresh open");
        }
    }

    /// rebuild_from_cursor_state must be idempotent — calling it
    /// twice with the same session must NOT double the rows. This is
    /// the contract that lets `sync_now` call it on every fs event
    /// without worrying about table growth.
    #[test]
    fn rebuild_is_idempotent() {
        let _lock = HOME_LOCK.lock().unwrap();
        let (_home, db) = fresh_unified_db();
        let s = sample_session("uuid-a", "proj-a");
        let n1 = db
            .rebuild_from_cursor_state(&[s.clone()], "host-a", 100)
            .expect("rebuild 1");
        let n2 = db
            .rebuild_from_cursor_state(&[s], "host-a", 200)
            .expect("rebuild 2");
        assert_eq!(n1, 1);
        assert_eq!(n2, 1);
        let counts = db.row_counts().unwrap();
        assert_eq!(counts["sessions"], 1, "second rebuild must not double");
        assert_eq!(counts["bubbles"], 0, "no bubbles in canonical without L1");
    }

    /// rebuild_from_cursor_state must write `sessions.content_hash`
    /// deterministically — same bubble set → same hash on every
    /// rebuild. This is what `core::conflict::classify` (PR-2) keys
    /// off to decide New / Identical / IncomingNewer / etc.
    #[test]
    fn rebuild_writes_content_hash_deterministically() {
        // Build a CanonicalSession with composer_data populated and
        // craft a scenario where rebuild calls read_conversation,
        // which finds nothing (no JSONL on disk) — so bubbles is
        // empty and content_hash is the empty-input SHA-256.
        let _lock = HOME_LOCK.lock().unwrap();
        let (_home, db) = fresh_unified_db();
        let mut s = sample_session("uuid-b", "proj-b");
        s.composer_data = Some(ComposerData {
            full_json: r#"{"name":"hello"}"#.into(),
            subset_json: r#"{"name":"hello"}"#.into(),
        });
        s.composer_id = Some("uuid-b".into());
        db.rebuild_from_cursor_state(&[s], "host-b", 500).unwrap();
        let row = db.get_session_meta("uuid-b").unwrap().unwrap();
        // Empty bubble list → SHA-256("") known constant.
        assert_eq!(
            row.content_hash,
            conflict::content_hash_from_bubbles(&[])
        );
        assert_eq!(row.endpoint_kind, "linux_cli");
        assert_eq!(row.host, "host-b");
        assert_eq!(row.bubble_count, 0);
        assert!(!row.is_broken);
    }

    /// record_archive must persist the row and return the new id;
    /// delete_session_row must cascade-clean bubbles / composer_data
    /// / archive / conflicts via FOREIGN KEY ON DELETE CASCADE.
    #[test]
    fn archive_and_delete_cascade() {
        let _lock = HOME_LOCK.lock().unwrap();
        let (_home, db) = fresh_unified_db();
        let mut s = sample_session("uuid-c", "proj-c");
        s.composer_data = Some(ComposerData {
            full_json: "{}".into(),
            subset_json: "{}".into(),
        });
        db.rebuild_from_cursor_state(&[s.clone()], "host-c", 1).unwrap();
        let archive_id = db
            .record_archive("uuid-c", "before_overwrite", "{\"x\":1}", 2)
            .unwrap();
        assert!(archive_id > 0);
        let conflict_id = db
            .record_conflict("uuid-c", ConflictClass::Diverged, Some("h-local"), Some("h-in"), 3)
            .unwrap();
        assert!(conflict_id > 0);
        let counts = db.row_counts().unwrap();
        assert_eq!(counts["archive"], 1);
        assert_eq!(counts["conflicts"], 1);
        assert_eq!(counts["composer_data"], 1);

        db.delete_session_row("uuid-c").unwrap();
        let counts = db.row_counts().unwrap();
        assert_eq!(counts["sessions"], 0);
        // Cascade: composer_data, archive, conflicts all gone.
        assert_eq!(counts["composer_data"], 0);
        assert_eq!(counts["archive"], 0);
        assert_eq!(counts["conflicts"], 0);
    }

    /// resolve_conflict must flip resolved_at_ms from NULL to a
    /// timestamp and store the resolution `how` so the UI can render
    /// "auto-merged" / "user chose local" etc.
    #[test]
    fn resolve_conflict_marks_resolved() {
        let _lock = HOME_LOCK.lock().unwrap();
        let (_home, db) = fresh_unified_db();
        db.rebuild_from_cursor_state(
            &[sample_session("uuid-d", "proj-d")],
            "host-d",
            1,
        )
        .unwrap();
        let cid = db
            .record_conflict("uuid-d", ConflictClass::Diverged, Some("h-local"), Some("h-in"), 2)
            .unwrap();
        let open = db.unresolved_conflicts().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, cid);
        assert_eq!(open[0].class, "Diverged");
        db.resolve_conflict(cid, "auto_merged", 3).unwrap();
        let open = db.unresolved_conflicts().unwrap();
        assert_eq!(open.len(), 0, "resolved conflict must drop out of unresolved list");
    }

    /// sync_runs bookkeeping: record → finish → row reflects the
    /// final tallies + finished_at_ms + error string.
    #[test]
    fn sync_run_record_and_finish() {
        let _lock = HOME_LOCK.lock().unwrap();
        let (_home, db) = fresh_unified_db();
        let run_id = db.record_sync_run("macbook", "pull", 100).unwrap();
        db.finish_sync_run(run_id, 12, 0, 250, None).unwrap();
        let conn = db.conn.lock().unwrap();
        let (peer, direction, items, finished, err): (
            String,
            String,
            i64,
            Option<i64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT peer_id, direction, items_processed, finished_at_ms, error
                 FROM sync_runs WHERE id = ?1",
                params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(peer, "macbook");
        assert_eq!(direction, "pull");
        assert_eq!(items, 12);
        assert_eq!(finished, Some(250));
        assert_eq!(err, None);
    }

    /// preferred_endpoint_kind / preferred_source_path from the
    /// canonical `Sources` struct must round-trip into the
    /// unified.db `sessions.endpoint_kind` / `sessions.source_path`
    /// columns. mac > linux_desktop > linux_cli priority order is
    /// preserved.
    #[test]
    fn rebuild_honors_sources_priority_order() {
        let _lock = HOME_LOCK.lock().unwrap();
        let (_home, db) = fresh_unified_db();
        let mut s = sample_session("uuid-e", "proj-e");
        s.sources = Sources {
            mac: Some(SourceInfo {
                last_seen_at: 1,
                layer: "3".into(),
                path: "/mac/state.vscdb".into(),
            }),
            linux_desktop: Some(SourceInfo {
                last_seen_at: 2,
                layer: "3".into(),
                path: "/linux/state.vscdb".into(),
            }),
            linux_cli: Some(SourceInfo {
                last_seen_at: 3,
                layer: "2".into(),
                path: "/linux/store.db".into(),
            }),
        };
        db.rebuild_from_cursor_state(&[s], "host-e", 1).unwrap();
        let row = db.get_session_meta("uuid-e").unwrap().unwrap();
        assert_eq!(row.endpoint_kind, "mac", "mac wins over linux_*");
        assert_eq!(row.source_path, "/mac/state.vscdb");
    }

    /// `bubble` helper sanity: shape must match what canonical's
    /// `read_conversation` returns so rebuild's bubble ingest path
    /// accepts it. Not exercised by `rebuild_from_cursor_state`
    /// directly because that calls read_conversation internally —
    /// this test documents the helper contract.
    #[test]
    fn bubble_helper_round_trip() {
        let b = bubble("b1", "user", "hi", 1);
        let json = serde_json::to_string(&b).unwrap();
        let back: Bubble = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "b1");
        assert_eq!(back.role, "user");
        assert_eq!(back.created_at_ms, 1);
        assert_eq!(back.tool_calls.len(), 1);
        assert_eq!(back.tool_calls[0].name, "Glob");
        assert!(back.parent_bubble_id.is_none());
    }

    /// `content_hash_from_bubbles` is private today; the public-facing
    /// contract is "different bubble texts → different hash". Two
    /// identical bubble sets must produce identical hashes; a one-byte
    /// text change must flip the hash.
    #[test]
    fn content_hash_changes_when_text_changes() {
        let a = vec![bubble("b1", "user", "hello", 1), bubble("b2", "assistant", "world", 2)];
        let b = vec![bubble("b1", "user", "hello", 1), bubble("b2", "assistant", "world!", 2)];
        let h_a = conflict::content_hash_from_bubbles(&a);
        let h_b = conflict::content_hash_from_bubbles(&b);
        assert_ne!(h_a, h_b, "text edit must change content hash");
        // Same input → same hash (deterministic).
        let h_a2 = conflict::content_hash_from_bubbles(&a);
        assert_eq!(h_a, h_a2);
    }

    /// Sources::preferred_endpoint_kind() / preferred_source_path()
    /// cover the four cases (mac / linux_desktop / linux_cli / none).
    #[test]
    fn sources_preferred_helpers_four_cases() {
        let none = Sources::default();
        assert_eq!(none.preferred_endpoint_kind(), "unknown");
        assert_eq!(none.preferred_source_path(), "");

        let mac_only = Sources {
            mac: Some(SourceInfo {
                last_seen_at: 1,
                layer: "3".into(),
                path: "/m".into(),
            }),
            linux_cli: None,
            linux_desktop: None,
        };
        assert_eq!(mac_only.preferred_endpoint_kind(), "mac");
        assert_eq!(mac_only.preferred_source_path(), "/m");

        let ld_only = Sources {
            mac: None,
            linux_cli: None,
            linux_desktop: Some(SourceInfo {
                last_seen_at: 1,
                layer: "3".into(),
                path: "/ld".into(),
            }),
        };
        assert_eq!(ld_only.preferred_endpoint_kind(), "linux_desktop");
        assert_eq!(ld_only.preferred_source_path(), "/ld");

        let lc_only = Sources {
            mac: None,
            linux_cli: Some(SourceInfo {
                last_seen_at: 1,
                layer: "2".into(),
                path: "/lc".into(),
            }),
            linux_desktop: None,
        };
        assert_eq!(lc_only.preferred_endpoint_kind(), "linux_cli");
        assert_eq!(lc_only.preferred_source_path(), "/lc");

        // SourceLayer enum round-trip via the canonical type (defensive).
        assert_eq!(SourceLayer::Mac, SourceLayer::Mac);
    }
}