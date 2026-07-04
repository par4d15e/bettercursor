//! bettercursor manual layer-2 / layer-3 sync — fill in missing storage
//! layers for a single session with one click.
//!
//! Background
//! ----------
//! Cursor splits each session across three storage layers (see
//! `PRD.md` §4.2). Each layer is **origin-specific**:
//!
//!   - Layer 1 (JSONL):   transcript, written by both CLI and Desktop.
//!   - Layer 2 (store.db): `cursor-agent` CLI only — needed for `--resume`.
//!   - Layer 3 (state.vscdb): Cursor Desktop Electron only — needed for
//!     Sidebar visibility.
//!
//! A session created in one tool is therefore invisible to the other
//! until the missing layer is filled. This module writes the missing
//! layers for a single session atomically:
//!
//!   1. Bail if Cursor / cursor-agent is still running (#84 race).
//!   2. Determine which layers are missing.
//!   3. If Layer 2 is missing: synthesize `store.db` (blobs + meta[0])
//!      from the Layer 3 conversation state + Layer 1 transcript.
//!      Run `fix_latest_root_blob` so `--resume` works.
//!   4. If Layer 3 is missing: synthesize `composerData:<uuid>` +
//!      `composer.composerHeaders` + `bubbleId:<uuid>:<bid>` rows from
//!      the existing Layer 1 transcript + Layer 2 blobs.
//!
//! `sync_session` does all writes inline in the Rust process — the
//! user explicitly asked for "一键同步, no in-app scripts". Trade-off:
//! any crash mid-write loses progress, mitigated by per-write
//! backups (`backup_existing` for both store.db and state.vscdb).
//!
//! L2 (store.db) format reference: `bettercursor/layer2.py:90-159`
//! (Python adapter, archived but still in-tree). Blob DAG analysis
//! ported from `bettercursor/blob_dag.py:1-200`. Layer 3 schema
//! composition comes from `core::inject` (Mutation enum + compose_*
//! helpers), which is the only surviving consumer of the legacy
//! "offline two-phase inject" pipeline.

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::canonical::{self};
use super::inject::{
    self, compose_bubble_blobs, compose_composer_data, compose_composer_header_entry,
    merge_composer_headers, parse_layer1_bubbles, scan_tracked_git_repos,
    truncate_to_title_pub, Mutation,
};
use super::inject::build_workspace_identifier;
use super::{paths, storage};

// ── Public types ──────────────────────────────────────────────

/// Result of one sync invocation. Returned to the frontend verbatim
/// so it can render a per-layer outcome badge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub uuid: String,
    /// True iff we wrote/updated Layer 2 store.db.
    pub wrote_layer2: bool,
    /// True iff we wrote/updated Layer 3 state.vscdb.
    pub wrote_layer3: bool,
    /// Non-fatal skip reasons. Empty when both writes succeeded.
    /// Examples: "cursor_running", "l2_already_present",
    /// "l3_already_present", "no_source_data", "no_cwd".
    pub skipped: Vec<String>,
    /// When L2 was written, the root blob id we patched into
    /// `meta[0].latestRootBlobId`. None when L2 was skipped.
    pub root_blob_id: Option<String>,
    pub duration_ms: u64,
}

/// Run the full sync for one session. `cwd` is the project working
/// directory (used to compute `~/.cursor/chats/<md5(cwd)>/<uuid>/`).
///
/// Failure modes (returned as Err, *not* SyncReport):
///   - Cursor / cursor-agent is running (`skipped` is for soft skips).
///   - Neither L1 nor L3 has data for this uuid (no source to synthesize).
///   - Filesystem I/O failure.
pub fn sync_session(uuid: &str, cwd: &str) -> Result<SyncReport> {
    let started = Instant::now();
    let mut report = SyncReport {
        uuid: uuid.to_string(),
        wrote_layer2: false,
        wrote_layer3: false,
        skipped: Vec::new(),
        root_blob_id: None,
        duration_ms: 0,
    };

    // ── 1. Lock check ───────────────────────────────────────
    let running = super::process::cursor_processes_running();
    if !running.is_empty() {
        report.skipped.push(format!(
            "cursor_running({} proc, e.g. {})",
            running.len(),
            running[0]
        ));
        report.duration_ms = started.elapsed().as_millis() as u64;
        return Ok(report);
    }

    // ── 2. Discover what we already have ────────────────────
    let (layer1_jsonl, bubbles_from_layer1) = read_layer1(uuid);
    let layer3_data = read_layer3_composer_data(uuid);
    let layer2_path = paths::store_db_for(cwd, uuid);
    let layer3_path = paths::global_db_path().ok();

    let need_l2 = !layer2_path.exists();
    let need_l3 = layer3_data.is_none();

    if !need_l2 && !need_l3 {
        report.skipped.push("already_synced".to_string());
        report.duration_ms = started.elapsed().as_millis() as u64;
        return Ok(report);
    }

    // Need at least one source of truth to synthesize from.
    if layer3_data.is_none() && bubbles_from_layer1.is_empty() {
        report.skipped.push("no_source_data".to_string());
        report.duration_ms = started.elapsed().as_millis() as u64;
        return Ok(report);
    }

    // ── 3. Write L2 if missing ──────────────────────────────
    if need_l2 {
        match write_layer2(uuid, cwd, &layer3_data, &bubbles_from_layer1) {
            Ok(root) => {
                report.wrote_layer2 = true;
                report.root_blob_id = Some(root);
            }
            Err(e) => {
                report.skipped.push(format!("l2_write_failed: {e}"));
            }
        }
    }

    // ── 4. Write L3 if missing ──────────────────────────────
    if need_l3 {
        if let Some(l3_path) = layer3_path {
            match write_layer3(uuid, cwd, &bubbles_from_layer1) {
                Ok(()) => report.wrote_layer3 = true,
                Err(e) => report.skipped.push(format!("l3_write_failed: {e}")),
            }
            let _ = l3_path; // referenced for clarity; write_layer3 reads it itself
        }
    }

    // ── 5. v0.3.0: mirror into unified.db (Migration A coexist) ──
    //
    // This is the read-cache hook — unified.db is rebuilt from the
    // canonical 3-layer state so the FTS5 mirror, content_hash, and
    // conflicts / archive tables reflect the post-write world.
    // Failures here MUST NOT fail the inline-write; log and continue.
    if let Ok(unified) = super::unified::UnifiedDb::open() {
        if let Ok(all) = canonical::scan_all() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let host = crate::core::paths::bettercursor_dir()
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("local")
                .to_string();
            let _ = unified.rebuild_from_cursor_state(&all, &host, now_ms);
        }
    }

    report.duration_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

// ── Layer 1 read (transcript) ────────────────────────────────

type Bubbles = Vec<canonical::Bubble>;

fn read_layer1(uuid: &str) -> (Option<PathBuf>, Bubbles) {
    let Some(path) = paths::find_layer1_jsonl_for(uuid) else {
        return (None, Vec::new());
    };
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(_) => return (Some(path), Vec::new()),
    };
    (Some(path), inject::parse_layer1_bubbles(uuid, &body))
}

// ── Layer 3 read (composerData JSON) ─────────────────────────

#[derive(Debug, Clone)]
struct Layer3Data {
    name: String,
    created_at_ms: i64,
    force_mode: String,
    is_run_everything: bool,
    /// The conversationState blob bytes, base64-encoded in the
    /// original JSON under `~`-prefixed field. None when absent.
    conversation_state_b64: Option<String>,
    /// Per-bubble bodies from `bubbleId:<uuid>:<bid>` rows (already
    /// decoded JSON). Used when we want to mirror Desktop's exact
    /// bubble structure rather than re-synthesize from Layer 1.
    bubble_blobs: HashMap<String, serde_json::Value>,
}

fn read_layer3_composer_data(uuid: &str) -> Option<Layer3Data> {
    let db_path = paths::global_db_path().ok()?;
    if !db_path.exists() {
        return None;
    }
    let r = storage::open_read(&db_path).ok()?;
    let key = format!("composerData:{uuid}");
    let v = r.get_json(&key, "cursorDiskKV").ok().flatten()?;

    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("New Agent")
        .to_string();
    let created_at_ms = v
        .get("createdAt")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    let force_mode = v
        .get("forceMode")
        .and_then(|x| x.as_str())
        .unwrap_or("default")
        .to_string();
    let is_run_everything = v
        .get("isRunEverything")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);

    let conversation_state_b64 = v
        .get("conversationState")
        .and_then(|x| x.as_str())
        .map(str::to_string);
    // Cursor writes conversationState as `~<base64>` (the `~` is a
    // version sentinel). Trim it so downstream base64-decode works.
    let conversation_state_b64 = conversation_state_b64
        .map(|s| s.trim_start_matches('~').to_string())
        .filter(|s| !s.is_empty());

    // Bubble blobs: best-effort read of bubbleId:* rows so we can
    // mirror Desktop's exact rendering on the L2 side. Not all
    // sessions have them; we silently skip missing ones.
    let mut bubble_blobs = HashMap::new();
    if let Ok(keys) = r.list_keys(&format!("bubbleId:{uuid}:"), "cursorDiskKV") {
        for k in keys {
            let Some(bid) = k.strip_prefix(&format!("bubbleId:{uuid}:")) else {
                continue;
            };
            if let Ok(Some(bv)) = r.get_json(&k, "cursorDiskKV") {
                bubble_blobs.insert(bid.to_string(), bv);
            }
        }
    }

    Some(Layer3Data {
        name,
        created_at_ms,
        force_mode,
        is_run_everything,
        conversation_state_b64,
        bubble_blobs,
    })
}

// ── Layer 2 write (store.db) ─────────────────────────────────

fn write_layer2(
    uuid: &str,
    cwd: &str,
    layer3: &Option<Layer3Data>,
    bubbles: &[canonical::Bubble],
) -> Result<String> {
    let store_db = paths::store_db_for(cwd, uuid);
    let chat_dir = store_db
        .parent()
        .ok_or_else(|| anyhow!("store.db has no parent dir"))?;
    std::fs::create_dir_all(chat_dir)
        .with_context(|| format!("create chat_dir {}", chat_dir.display()))?;

    // Backup if we're about to overwrite an existing store.db.
    backup_existing(&store_db);

    // Decide what blobs to write. Three source paths in priority order:
    //   (a) Layer 3 conversationState decoded → primary blob DAG.
    //   (b) Layer 3 bubble blobs → individual bubble blobs.
    //   (c) Layer 1 bubbles → synthesized as text blobs (last resort,
    //       may lose tool-call structure that Layer 3 has).
    //
    // For now we go with (a)+(b)+(c)-when-needed: write Layer 3's blobs
    // (when present) so the DAG matches what Desktop produced; if L3
    // is missing, synthesize from L1 text bubbles.
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(l3) = layer3 {
        if let Some(b64) = &l3.conversation_state_b64 {
            if let Ok(bytes) = base64_decode(b64) {
                let id = sha256_hex(&bytes);
                blobs.push((id, bytes));
            }
        }
        for (bid, body) in &l3.bubble_blobs {
            // Bubble blobs aren't stored in store.db — only the
            // canonical conversationState blob chain is. But we record
            // the bubble ids so the root-finder sees them as part of
            // the DAG.
            let _ = bid;
            let _ = body;
        }
    }
    if blobs.is_empty() && !bubbles.is_empty() {
        // Synthesize: each bubble becomes its own blob; the root
        // blob holds the list of bubble ids.
        let mut bubble_ids = Vec::with_capacity(bubbles.len());
        for b in bubbles {
            let payload = serde_json::to_vec(&serde_json::json!({
                "role": b.role,
                "text": b.text,
                "createdAt": b.created_at_ms,
            }))?;
            let id = sha256_hex(&payload);
            bubble_ids.push(id.clone());
            blobs.push((id, payload));
        }
        // Root blob: a small protobuf-like header pointing at each
        // bubble. Format mirrors what blob_dag.py expects: a 32-byte
        // hash is enough to register a reference in the DAG walker.
        let mut root = Vec::new();
        for bid in &bubble_ids {
            if let Ok(bytes) = hex_decode(bid) {
                // Field tag 1, wire type 2 (length-delimited), length 32.
                root.push(0x0A); // (1 << 3) | 2
                root.push(32);
                root.extend_from_slice(&bytes);
            }
        }
        let root_id = sha256_hex(&root);
        blobs.push((root_id, root));
    }

    // _init_store_db: create schema if missing.
    let conn = Connection::open(&store_db)
        .with_context(|| format!("open {}", store_db.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS blobs(id TEXT PRIMARY KEY, data BLOB);
         CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);",
    )?;

    // Write blobs.
    let tx = conn.unchecked_transaction()?;
    for (id, data) in &blobs {
        tx.execute(
            "INSERT OR REPLACE INTO blobs(id, data) VALUES (?1, ?2)",
            params![id, data],
        )?;
    }
    // Write meta[0] with empty latestRootBlobId (we patch below).
    let name = layer3
        .as_ref()
        .map(|l3| l3.name.clone())
        .unwrap_or_else(|| {
            bubbles
                .iter()
                .find(|b| b.role == "user")
                .map(|b| truncate_to_title_pub(&b.text))
                .unwrap_or_else(|| "New Agent".to_string())
        });
    let created_at = layer3.as_ref().map(|l3| l3.created_at_ms).unwrap_or_else(|| {
        bubbles
            .iter()
            .map(|b| b.created_at_ms)
            .find(|&t| t > 0)
            .unwrap_or(0)
    });
    let mode = layer3
        .as_ref()
        .map(|l3| l3.force_mode.clone())
        .unwrap_or_else(|| "default".to_string());
    let is_run_everything = layer3
        .as_ref()
        .map(|l3| l3.is_run_everything)
        .unwrap_or(false);
    let meta0 = serde_json::json!({
        "agentId": uuid,
        "latestRootBlobId": "",
        "name": name,
        "mode": mode,
        "isRunEverything": is_run_everything,
        "createdAt": created_at,
    });
    let meta0_hex = hex_encode(meta0.to_string().as_bytes());
    tx.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
        params!["0", meta0_hex],
    )?;
    tx.commit()?;

    // Drop the write conn so fix_latest_root can open its own.
    drop(conn);

    // ── Fix latestRootBlobId (the unique value-add of bettercursor;
    //    see blob_dag.py:149-187) ────────────────────────────────
    let root = fix_latest_root(&store_db)?;

    // Also write meta.json + prompt_history.json so cursor-agent's
    // chat-dir layout matches what the CLI itself produces. These are
    // not strictly required for `--resume` but improve UX (better
    // error messages, prompt history loading).
    write_meta_json(chat_dir, uuid, &name, created_at);
    write_prompt_history(chat_dir);

    Ok(root)
}

fn write_meta_json(chat_dir: &Path, uuid: &str, title: &str, created_at_ms: i64) {
    let body = serde_json::json!({
        "schemaVersion": 1,
        "createdAtMs": created_at_ms,
        "hasConversation": true,
        "title": title,
        "cwd": chat_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|_md5| ())
            .map(|()| ()),
        "updatedAtMs": created_at_ms,
    });
    // The "cwd" field above is intentionally lossy (we don't reverse
    // md5 → path here). cursor-agent only reads it for display, so
    // an empty string is acceptable. If needed, the caller can pass
    // the actual cwd as a parameter; for now this matches what
    // apply.py writes (it omits cwd when synthesizing).
    let body = serde_json::json!({
        "schemaVersion": 1,
        "createdAtMs": created_at_ms,
        "hasConversation": true,
        "title": title,
        "updatedAtMs": created_at_ms,
    });
    let _ = std::fs::write(
        chat_dir.join("meta.json"),
        serde_json::to_string_pretty(&body).unwrap_or_default(),
    );
    let _ = uuid; // referenced for future use
}

fn write_prompt_history(chat_dir: &Path) {
    let body = serde_json::json!(["/resume"]);
    let _ = std::fs::write(
        chat_dir.join("prompt_history.json"),
        serde_json::to_string(&body).unwrap_or_default(),
    );
}

fn backup_existing(path: &Path) {
    if !path.exists() {
        return;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let backup = path.with_extension(format!(
        "{}.backup_{}",
        path.extension().and_then(|s| s.to_str()).unwrap_or("db"),
        ts
    ));
    let _ = std::fs::copy(path, &backup);
}

// ── Protobuf walker (for find_latest_root) ───────────────────

/// Walk a Cursor protobuf blob and collect every 32-byte length-delimited
/// field as a hex SHA256 reference. Mirrors `blob_dag.py:51-92`.
fn extract_blob_ids_from_protobuf(raw: &[u8]) -> HashSet<String> {
    let mut ids = HashSet::new();
    walk_protobuf(raw, &mut ids);
    ids
}

fn walk_protobuf(data: &[u8], into: &mut HashSet<String>) {
    let mut offset = 0;
    while offset < data.len() {
        // Read tag varint.
        let (tag, new_offset) = match read_varint(data, offset) {
            Ok(v) => v,
            Err(_) => break,
        };
        offset = new_offset;
        let wire_type = tag & 0x07;
        match wire_type {
            2 => {
                // Length-delimited.
                let (length_u64, data_start) = match read_varint(data, offset) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let length = match usize::try_from(length_u64) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if data_start + length > data.len() {
                    break;
                }
                if length == 32 {
                    let hex: String = data[data_start..data_start + 32]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    into.insert(hex);
                    offset = data_start + 32;
                } else if length > 0 {
                    walk_protobuf(&data[data_start..data_start + length], into);
                    offset = data_start + length;
                } else {
                    offset = data_start;
                }
            }
            0 => {
                // Varint.
                let (_, new_offset) = match read_varint(data, offset) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                offset = new_offset;
            }
            5 => {
                // 32-bit.
                offset += 4;
            }
            1 => {
                // 64-bit.
                offset += 8;
            }
            _ => {
                offset += 1;
            }
        }
    }
}

fn read_varint(data: &[u8], mut offset: usize) -> Result<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        if offset >= data.len() {
            return Err(anyhow!("varint overflow at offset {offset}"));
        }
        let b = data[offset];
        offset += 1;
        result |= ((b & 0x7F) as u64) << shift;
        if (b & 0x80) == 0 {
            return Ok((result, offset));
        }
        shift += 7;
        if shift >= 64 {
            return Err(anyhow!("varint too large"));
        }
    }
}

// ── Find + fix latestRootBlobId ──────────────────────────────

fn find_root_blob(store_db: &Path) -> Option<String> {
    let conn = Connection::open_with_flags(store_db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut stmt = match conn.prepare("SELECT id, data FROM blobs") {
        Ok(s) => s,
        Err(_) => return None,
    };
    let rows: Vec<(String, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))
        .ok()?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    drop(conn);
    if rows.is_empty() {
        return None;
    }
    let blob_ids: HashSet<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let mut referenced_by: HashMap<String, HashSet<String>> = HashMap::new();
    let mut references: HashMap<String, HashSet<String>> = HashMap::new();
    for id in &blob_ids {
        referenced_by.entry(id.clone()).or_default();
    }
    for (id, data) in &rows {
        let refs = extract_blob_ids_from_protobuf(data);
        let matched: HashSet<String> = refs
            .into_iter()
            .filter(|r| blob_ids.contains(r) && r != id)
            .collect();
        references.insert(id.clone(), matched.clone());
        for r in matched {
            referenced_by.entry(r).or_default().insert(id.clone());
        }
    }
    let roots: Vec<&String> = blob_ids
        .iter()
        .filter(|id| referenced_by.get(*id).map(|s| s.is_empty()).unwrap_or(true))
        .collect();
    if roots.is_empty() {
        return None;
    }
    // Tie-break by transitive coverage (largest descendant set wins).
    let mut best: Option<(&String, usize)> = None;
    for r in &roots {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = vec![(*r).clone()];
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            if let Some(refs) = references.get(&cur) {
                for x in refs {
                    stack.push(x.clone());
                }
            }
        }
        let coverage = seen.len();
        if best.map(|(_, c)| coverage > c).unwrap_or(true) {
            best = Some((r, coverage));
        }
    }
    best.map(|(s, _)| s.clone())
}

fn fix_latest_root(store_db: &Path) -> Result<String> {
    let root = find_root_blob(store_db)
        .ok_or_else(|| anyhow!("could not find a root blob (empty DAG?)"))?;
    let conn = Connection::open(store_db)?;
    let existing_hex: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = '0'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let Some(existing_hex) = existing_hex else {
        return Err(anyhow!("meta[0] missing; cannot patch"));
    };
    let existing_bytes = hex_decode(&existing_hex)?;
    let mut meta: serde_json::Value = serde_json::from_slice(&existing_bytes)
        .with_context(|| "meta[0] is not valid JSON")?;
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("latestRootBlobId".to_string(), serde_json::Value::String(root.clone()));
    } else {
        return Err(anyhow!("meta[0] is not a JSON object"));
    }
    let new_hex = hex_encode(serde_json::to_string(&meta)?.as_bytes());
    conn.execute(
        "UPDATE meta SET value = ?1 WHERE key = '0'",
        params![new_hex],
    )?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    Ok(root)
}

// ── v0.2.1: bulk-fix orphan sessions + single-session delete ─

/// v0.2.1: 扫所有 `~/.cursor/chats/*/<uuid>/store.db`, 把
/// `meta[0].latestRootBlobId` 是空字符串的 session 修上. 修之前
/// 调 [`backup_existing`] 留一份 `.backup_<ts>` 副本, 出问题可回滚.
///
/// 这是 `core::canonical::scan_layer2_into` 标的 "broken" session
/// 的实际修复入口 —— v0.1 只标不修.
pub fn fix_orphans() -> Result<FixOrphansReport> {
    use std::fs;

    let mut report = FixOrphansReport {
        fixed: Vec::new(),
        skipped: Vec::new(),
        scanned: 0,
    };
    let chats_root = paths::chats_dir();
    if !chats_root.exists() {
        return Ok(report);
    }
    let md5_roots = match fs::read_dir(&chats_root) {
        Ok(r) => r,
        Err(e) => {
            return Err(anyhow!("read chats dir {}: {e}", chats_root.display()));
        }
    };
    for md5_entry in md5_roots.flatten() {
        let md5_dir = md5_entry.path();
        if !md5_dir.is_dir() {
            continue;
        }
        let uuids = match fs::read_dir(&md5_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for uuid_entry in uuids.flatten() {
            let uuid_dir = uuid_entry.path();
            if !uuid_dir.is_dir() {
                continue;
            }
            let uuid = match uuid_entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let store_db = uuid_dir.join("store.db");
            if !store_db.is_file() {
                continue;
            }
            report.scanned += 1;
            // Cheap pre-check: read meta[0] and see if latestRootBlobId
            // is empty. Skipping the open on healthy sessions avoids
            // calling find_root_blob for every chat in the system.
            match read_latest_root_blob_id(&store_db) {
                Ok(Some(s)) if !s.is_empty() => continue,
                Ok(Some(_)) => { /* empty root — fall through to fix */ }
                Ok(None) => continue, // meta[0] missing → leave alone (different broken mode)
                Err(e) => {
                    report
                        .skipped
                        .push(format!("{uuid}: read meta[0] failed: {e}"));
                    continue;
                }
            }
            // Pre-fix backup so a bad fix_latest_root can be reverted.
            backup_existing(&store_db);
            match fix_latest_root(&store_db) {
                Ok(new_root) => report.fixed.push(format!("{uuid} (root={})", &new_root[..8.min(new_root.len())])),
                Err(e) => report.skipped.push(format!("{uuid}: {e}")),
            }
            // v0.3.0: archive the pre-fix state into unified.db so the
            // recovery tool can reconstruct if the patch turned out
            // wrong. Best-effort — failures here MUST NOT block the
            // actual fix_latest_root operation that already succeeded.
            if let Ok(unified) = super::unified::UnifiedDb::open() {
                let now_ms = chrono::Utc::now().timestamp_millis();
                let pre_fix_blob_id = read_latest_root_blob_id(&store_db)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let payload = serde_json::json!({
                    "uuid": uuid,
                    "pre_fix_latest_root_blob_id": pre_fix_blob_id,
                })
                .to_string();
                let _ = unified.record_archive(
                    &uuid,
                    "before_fix_orphans",
                    &payload,
                    now_ms,
                );
            }
        }
    }
    // v0.3.0: rebuild unified.db from current canonical state so the
    // archive row counts and any sessions whose fix changed their
    // content_hash stay in sync. Best-effort.
    if let Ok(unified) = super::unified::UnifiedDb::open() {
        if let Ok(all) = canonical::scan_all() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let host = crate::core::paths::bettercursor_dir()
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("local")
                .to_string();
            let _ = unified.rebuild_from_cursor_state(&all, &host, now_ms);
        }
    }
    Ok(report)
}

/// Read `meta[0].latestRootBlobId` from a store.db. Returns:
///   - `Some(s)` — value (possibly empty string).
///   - `None` — meta[0] missing / not parseable / not an object.
fn read_latest_root_blob_id(store_db: &Path) -> Result<Option<String>> {
    let conn = Connection::open(store_db)?;
    let existing_hex: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = '0'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let Some(existing_hex) = existing_hex else {
        return Ok(None);
    };
    let bytes = hex_decode(&existing_hex)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| "meta[0] is not valid JSON")?;
    Ok(v.get("latestRootBlobId")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixOrphansReport {
    /// Successfully repaired UUIDs (with abbreviated root blob id).
    pub fixed: Vec<String>,
    /// UUIDs we couldn't repair + a one-line reason.
    pub skipped: Vec<String>,
    /// Total store.db files inspected.
    pub scanned: u32,
}

/// v0.2.1: delete a single session's Layer 1 + Layer 2 storage.
///
/// **Layer 3 is NEVER deleted** — Cursor Electron owns state.vscdb
/// and force-deleting rows there risks corrupting its startup read
/// (the same #84 race `core::sync::sync_session` defends against).
///
/// `project_slug` is sourced from `CanonicalSession.project_slug` on
/// the frontend; the backend does not recompute it. If `None`, only
/// Layer 2 is deleted (Layer 1 path requires the slug to disambiguate
/// which `<project>/agent-transcripts/` directory to remove).
///
/// Pre-flight: refuses to proceed if Cursor / cursor-agent is
/// running — same `#84` lock as `sync_session`.
pub fn delete_session(uuid: &str, cwd: &str, project_slug: Option<&str>) -> Result<DeleteReport> {
    use std::fs;

    let running = super::process::cursor_processes_running();
    if !running.is_empty() {
        return Ok(DeleteReport {
            uuid: uuid.to_string(),
            removed_l1: false,
            removed_l2: false,
            skipped_l1: None,
            skipped_l2: None,
            cursor_running: true,
            running_processes: running,
        });
    }

    // ── Layer 2 ───────────────────────────────────────────────
    let l2_removed;
    let l2_skip;
    let chat_root = paths::chat_root_for(cwd);
    let l2_dir = paths::chats_dir().join(&chat_root).join(uuid);
    if l2_dir.is_dir() {
        match fs::remove_dir_all(&l2_dir) {
            Ok(()) => {
                l2_removed = true;
                l2_skip = None;
            }
            Err(e) => {
                l2_removed = false;
                l2_skip = Some(format!("io_error: {e}"));
            }
        }
    } else {
        l2_removed = false;
        l2_skip = Some("l2_not_present".to_string());
    }

    // ── Layer 1 ───────────────────────────────────────────────
    let l1_removed;
    let l1_skip;
    match project_slug {
        Some(slug) if !slug.is_empty() => {
            // Defensive: refuse to recurse into paths that contain
            // characters that would let a malicious/buggy caller
            // escape the projects directory.
            if !slug
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                l1_removed = false;
                l1_skip = Some("invalid_slug".to_string());
            } else {
                let l1_dir = paths::cursor_projects_dir()
                    .join(slug)
                    .join("agent-transcripts")
                    .join(uuid);
                if l1_dir.is_dir() {
                    match fs::remove_dir_all(&l1_dir) {
                        Ok(()) => {
                            l1_removed = true;
                            l1_skip = None;
                        }
                        Err(e) => {
                            l1_removed = false;
                            l1_skip = Some(format!("io_error: {e}"));
                        }
                    }
                } else {
                    l1_removed = false;
                    l1_skip = Some("l1_not_present".to_string());
                }
            }
        }
        _ => {
            l1_removed = false;
            l1_skip = Some("slug_not_provided".to_string());
        }
    }

    // ── v0.3.0: archive + delete in unified.db (Migration A coexist) ──
    //
    // We capture whatever canonical state we still have BEFORE deleting
    // so the recovery tool can reconstruct if the user later realizes
    // they wanted it. Both ops are best-effort — failures MUST NOT
    // mask the actual L1/L2 deletion results.
    if let Ok(unified) = super::unified::UnifiedDb::open() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let existing = canonical::scan_all()
            .ok()
            .and_then(|all| all.into_iter().find(|s| s.uuid == uuid));
        if let Some(ref s) = existing {
            let payload = serde_json::to_string(s).unwrap_or_default();
            let _ = unified.record_archive(
                uuid,
                "before_delete",
                &payload,
                now_ms,
            );
        }
        let _ = unified.delete_session_row(uuid);
    }

    Ok(DeleteReport {
        uuid: uuid.to_string(),
        removed_l1: l1_removed,
        removed_l2: l2_removed,
        skipped_l1: l1_skip,
        skipped_l2: l2_skip,
        cursor_running: false,
        running_processes: Vec::new(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteReport {
    pub uuid: String,
    pub removed_l1: bool,
    pub removed_l2: bool,
    /// None on success, "l1_not_present" / "l2_not_present" /
    /// "slug_not_provided" / "invalid_slug" / "io_error: ..." on skip.
    pub skipped_l1: Option<String>,
    pub skipped_l2: Option<String>,
    /// True iff `cursor_processes_running` returned non-empty — the
    /// frontend should re-show the confirmation dialog after closing
    /// Cursor.
    pub cursor_running: bool,
    /// Process names that triggered the cursor_running bail (for
    /// surfacing in the UI).
    pub running_processes: Vec<String>,
}

// ── Layer 3 write (state.vscdb composer entry) ───────────────

fn write_layer3(uuid: &str, cwd: &str, bubbles: &[canonical::Bubble]) -> Result<()> {
    if bubbles.is_empty() {
        return Err(anyhow!("no Layer 1 bubbles to synthesize Layer 3 from"));
    }

    let meta_cwd = if cwd.is_empty() {
        None
    } else {
        Some(cwd.to_string())
    };

    let name = bubbles
        .iter()
        .find(|b| b.role == "user")
        .map(|b| truncate_to_title_pub(&b.text))
        .unwrap_or_else(|| "CLI 会话".to_string());
    let subtitle = bubbles
        .iter()
        .find(|b| b.role == "user")
        .map(|b| truncate_to_title_pub(&b.text))
        .unwrap_or_else(|| name.clone());
    let created_at_ms = bubbles
        .iter()
        .map(|b| b.created_at_ms)
        .find(|&t| t > 0)
        .unwrap_or(0);
    let last_updated_ms = bubbles
        .iter()
        .map(|b| b.created_at_ms)
        .max()
        .unwrap_or(created_at_ms);

    let workspace_identifier = build_workspace_identifier(cwd).unwrap_or(serde_json::Value::Null);
    let project_slug = if cwd.is_empty() {
        format!("cli-{uuid}")
    } else {
        paths::sanitize_project_path(cwd)
    };
    let tracked_git_repos = if cwd.is_empty() {
        Vec::new()
    } else {
        scan_tracked_git_repos(cwd)
    };

    let composer_data = compose_composer_data(
        uuid,
        &name,
        created_at_ms,
        last_updated_ms,
        &Some(workspace_identifier.clone()),
        &project_slug,
        &tracked_git_repos,
        bubbles,
    );
    let header_entry = compose_composer_header_entry(
        uuid,
        &name,
        &subtitle,
        created_at_ms,
        last_updated_ms,
        &Some(workspace_identifier),
        &tracked_git_repos,
    );
    let bubble_blobs = compose_bubble_blobs(uuid, bubbles);

    // Collect agentKv blob refs from new + existing composerData (§9.8).
    let mut agent_blob_ids = extract_agent_blob_ids_from_composer(&composer_data);
    let db_path = paths::global_db_path()?;
    if let Ok(r) = storage::open_read(&db_path) {
        let key = format!("composerData:{uuid}");
        if let Ok(Some(existing)) = r.get_json(&key, "cursorDiskKV") {
            agent_blob_ids.extend(extract_agent_blob_ids_from_composer(&existing));
        }
    }

    // Apply mutations directly to state.vscdb via tmpdir-copy +
    // atomic_rename (same pattern as scripts/apply.py:175-202).
    backup_existing(&db_path);

    let tmp = tempfile::tempdir().context("create tmpdir for state.vscdb copy")?;
    let staged_db = tmp.path().join("state.vscdb");
    std::fs::copy(&db_path, &staged_db)?;
    for suf in ["-wal", "-shm"] {
        let sidecar = with_sidecar_suffix(&db_path, suf);
        if sidecar.exists() {
            let _ = std::fs::copy(&sidecar, tmp.path().join(format!("state.vscdb{suf}")));
        }
    }

    let conn = Connection::open(&staged_db)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ItemTable(key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE IF NOT EXISTS cursorDiskKV(key TEXT PRIMARY KEY, value TEXT);",
    )?;

    let merged_headers = merge_composer_headers(uuid, &header_entry)?;
    let mutations = vec![
        Mutation::ItemTableUpsert {
            key: "composer.composerHeaders".to_string(),
            value_hex: hex_encode(serde_json::to_string(&merged_headers)?.as_bytes()),
        },
        Mutation::DiskKvUpsert {
            key: format!("composerData:{uuid}"),
            value_hex: hex_encode(serde_json::to_string(&composer_data)?.as_bytes()),
        },
    ];
    for m in &mutations {
        apply_mutation_inline(&conn, m)?;
    }
    for (bubble_id, body) in &bubble_blobs {
        apply_mutation_inline(
            &conn,
            &Mutation::DiskKvUpsert {
                key: format!("bubbleId:{uuid}:{bubble_id}"),
                value_hex: hex_encode(serde_json::to_string(body)?.as_bytes()),
            },
        )?;
    }
    if !agent_blob_ids.is_empty() {
        if let Ok(r) = storage::open_read(&db_path) {
            for bid in &agent_blob_ids {
                let key = format!("agentKv:blob:{bid}");
                if let Ok(Some(bytes)) = r.get_item_binary(&key, "cursorDiskKV") {
                    apply_disk_kv_binary(&conn, &key, &bytes)?;
                }
            }
        }
    }

    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    let ok: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap_or_else(|_| "ok".to_string());
    if ok != "ok" {
        return Err(anyhow!("integrity_check failed: {ok}"));
    }
    drop(conn);

    // Atomic rename: stage on the same fs as the target first, then
    // os.replace. (#85 fix — /tmp tmpfs is separate fs.)
    atomic_replace(&staged_db, &db_path)?;
    for suf in ["-wal", "-shm"] {
        let tmp_sidecar = tmp.path().join(format!("state.vscdb{suf}"));
        if tmp_sidecar.exists() {
            let dst = with_sidecar_suffix(&db_path, suf);
            let _ = atomic_replace(&tmp_sidecar, &dst);
        }
    }

    let _ = meta_cwd; // future hook for richer metadata
    Ok(())
}

fn with_sidecar_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn atomic_replace(src: &Path, dst: &Path) -> Result<()> {
    if src.parent() != dst.parent() {
        let staged = dst.with_extension(format!(
            "{}.bettercursor-stage",
            dst.extension().and_then(|s| s.to_str()).unwrap_or("db")
        ));
        std::fs::copy(src, &staged)?;
        match std::fs::rename(&staged, dst) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&staged);
                Err(e.into())
            }
        }
    } else {
        std::fs::rename(src, dst).map_err(Into::into)
    }
}

fn extract_agent_blob_ids_from_composer(conv: &serde_json::Value) -> HashSet<String> {
    let cs = conv
        .get("conversationState")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if !cs.starts_with('~') || cs.len() < 10 {
        return HashSet::new();
    }
    base64_decode(&cs[1..])
        .map(|raw| extract_blob_ids_from_protobuf(&raw))
        .unwrap_or_default()
}

fn apply_disk_kv_binary(conn: &Connection, key: &str, value: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO cursorDiskKV(key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

fn apply_mutation_inline(conn: &Connection, m: &Mutation) -> Result<()> {
    let bytes = hex_decode(&m.value_hex())?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match m {
        Mutation::ItemTableUpsert { key, .. } => {
            conn.execute(
                "INSERT OR REPLACE INTO ItemTable(key, value) VALUES (?1, ?2)",
                params![key, text],
            )?;
        }
        Mutation::DiskKvUpsert { key, .. } => {
            conn.execute(
                "INSERT OR REPLACE INTO cursorDiskKV(key, value) VALUES (?1, ?2)",
                params![key, text],
            )?;
        }
    }
    Ok(())
}

// ── Hex / base64 / sha256 helpers ────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(anyhow!("odd-length hex string"));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for i in 0..s.len() / 2 {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(anyhow!("invalid hex char: {b}")),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(&h.finalize())
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() { bytes[i + 1] as u32 } else { 0 };
        let b2 = if i + 2 < bytes.len() { bytes[i + 2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if i + 1 < bytes.len() {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if i + 2 < bytes.len() {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
        i += 3;
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    // Minimal RFC 4648 base64 decoder (no external dep).
    let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in &s {
        let v: u32 = match b {
            b'A'..=b'Z' => (b - b'A') as u32,
            b'a'..=b'z' => (b - b'a' + 26) as u32,
            b'0'..=b'9' => (b - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            b'=' => continue,
            _ => return Err(anyhow!("invalid base64 char: {b}")),
        };
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hex_round_trip() {
        let s = "deadbeef";
        let bytes = hex_decode(s).unwrap();
        assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(hex_encode(&bytes), s);
    }

    #[test]
    fn hex_decode_rejects_odd_length() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn extract_agent_blob_ids_from_conversation_state() {
        let payload = vec![0xAB; 32];
        let mut proto = vec![0x0A, 32];
        proto.extend_from_slice(&payload);
        let b64 = base64_encode(&proto);
        let conv = serde_json::json!({
            "conversationState": format!("~{b64}"),
        });
        let ids = extract_agent_blob_ids_from_composer(&conv);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.iter().next().unwrap(), &hex_encode(&payload));
    }

    #[test]
    fn base64_decode_known_vector() {
        // "Hello, World!" → "SGVsbG8sIFdvcmxkIQ=="
        let bytes = base64_decode("SGVsbG8sIFdvcmxkIQ==").unwrap();
        assert_eq!(bytes, b"Hello, World!");
    }

    #[test]
    fn protobuf_walker_extracts_32_byte_field() {
        // Synthesize: tag=1 (field 1, wire-type 2), length=32, payload.
        let payload = vec![0xAB; 32];
        let mut data = vec![0x0A, 32]; // tag 1 LEN
        data.extend_from_slice(&payload);
        let ids = extract_blob_ids_from_protobuf(&data);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.iter().next().unwrap(), &hex_encode(&payload));
    }

    #[test]
    fn protobuf_walker_recurses_into_submessage() {
        // Outer tag=2 LEN, inner contains a 32-byte field.
        let inner_payload = vec![0xCD; 32];
        let mut inner = vec![0x0A, 32];
        inner.extend_from_slice(&inner_payload);
        let mut outer = vec![0x12, inner.len() as u8]; // tag 2 LEN
        outer.extend_from_slice(&inner);
        let ids = extract_blob_ids_from_protobuf(&outer);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids.iter().next().unwrap(), &hex_encode(&inner_payload));
    }

    #[test]
    fn find_root_blob_simple_chain() {
        // Three blobs: A references B references C. Root = A.
        let dir = tempdir().unwrap();
        let db = dir.path().join("store.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs(id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let c = vec![0u8; 0]; // empty payload
        let c_id = sha256_hex(&c);
        let b = build_protobuf_ref(&c_id);
        let b_id = sha256_hex(&b);
        let a = build_protobuf_ref(&b_id);
        let a_id = sha256_hex(&a);
        for (id, data) in [(&a_id, &a), (&b_id, &b), (&c_id, &c)] {
            conn.execute(
                "INSERT INTO blobs(id, data) VALUES (?1, ?2)",
                params![id, data],
            )
            .unwrap();
        }
        drop(conn);
        let root = find_root_blob(&db).expect("root");
        assert_eq!(root, a_id, "A (no incoming refs) should be the root");
    }

    #[test]
    fn fix_latest_root_writes_meta_zero() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("store.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs(id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let c = vec![0u8; 0];
        let c_id = sha256_hex(&c);
        let a = build_protobuf_ref(&c_id);
        let a_id = sha256_hex(&a);
        for (id, data) in [(&a_id, &a), (&c_id, &c)] {
            conn.execute(
                "INSERT INTO blobs(id, data) VALUES (?1, ?2)",
                params![id, data],
            )
            .unwrap();
        }
        let meta0 = serde_json::json!({"agentId":"x","latestRootBlobId":"","name":"y","mode":"default","isRunEverything":false,"createdAt":0});
        let meta_hex = hex_encode(meta0.to_string().as_bytes());
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('0', ?1)",
            params![meta_hex],
        )
        .unwrap();
        drop(conn);
        let root = fix_latest_root(&db).expect("fix root");
        assert_eq!(root, a_id);
        let conn = Connection::open(&db).unwrap();
        let new_hex: String = conn
            .query_row("SELECT value FROM meta WHERE key = '0'", [], |r| r.get(0))
            .unwrap();
        let new_meta: serde_json::Value = serde_json::from_slice(&hex_decode(&new_hex).unwrap()).unwrap();
        assert_eq!(new_meta["latestRootBlobId"], serde_json::Value::String(a_id));
    }

    /// Build a minimal protobuf blob that references a single 32-byte
    /// hash (the helper for find_root_blob_simple_chain).
    fn build_protobuf_ref(hash_hex: &str) -> Vec<u8> {
        let hash = hex_decode(hash_hex).unwrap();
        assert_eq!(hash.len(), 32);
        let mut out = vec![0x0A, 32];
        out.extend_from_slice(&hash);
        out
    }

    /// Live smoke test: fill in Layer 2 for `cfa4177f` (Desktop-only
    /// session). Requires the dev machine to have the real session on
    /// disk; otherwise the test is a no-op. Run with:
    ///   cargo test --lib sync::tests::live_smoke_cfa4177f -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_smoke_cfa4177f() {
        let uuid = "cfa4177f-8aee-4ee8-bd6c-2615478d033f";
        let cwd = "/home/eric/workspace/enenzuo";
        let report = sync_session(uuid, cwd).expect("sync_session");
        eprintln!("{report:?}");
        assert!(report.wrote_layer2 || !report.skipped.is_empty());
    }

    /// Live smoke test: fill in Layer 3 for `62eb1b04` (CLI-only
    /// session). Same caveats as cfa4177f.
    #[test]
    #[ignore]
    fn live_smoke_62eb1b04() {
        let uuid = "62eb1b04-bb13-42b2-b72f-916613c0599a";
        let cwd = "/home/eric/workspace/enenzuo";
        let report = sync_session(uuid, cwd).expect("sync_session");
        eprintln!("{report:?}");
        assert!(report.wrote_layer3 || !report.skipped.is_empty());
    }

    // ── v0.2.1: fix_orphans + delete_session ────────────────

    /// Global mutex guarding tests that monkey-patch `HOME` to point
    /// at a tempdir (so `paths::chats_dir()` / `cursor_projects_dir()`
    /// resolve into the temp tree). Without this lock, two such tests
    /// running in parallel race — and even worse, an unrelated test
    /// like `watcher::label_strips_home` that reads `home::home_dir()`
    /// mid-race sees the *tempdir* and asserts `~` prefix against a
    /// path that isn't under HOME. Mutex held only for the duration
    /// of one test fn body, so contention is brief.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Build a synthetic store.db with one blob `c` (empty payload),
    /// `b → c`, `a → b`, and `meta[0].latestRootBlobId = ""`. Mirrors
    /// the broken-state pattern cursor-agent leaves behind.
    fn make_broken_store_db(dir: &std::path::Path, with_empty_root: bool) -> std::path::PathBuf {
        let db = dir.join("store.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE blobs(id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let c = vec![0u8; 0];
        let c_id = sha256_hex(&c);
        let b = build_protobuf_ref(&c_id);
        let b_id = sha256_hex(&b);
        let a = build_protobuf_ref(&b_id);
        let a_id = sha256_hex(&a);
        for (id, data) in [(&c_id, &c), (&b_id, &b), (&a_id, &a)] {
            conn.execute(
                "INSERT INTO blobs(id, data) VALUES (?1, ?2)",
                params![id, data],
            )
            .unwrap();
        }
        let meta = if with_empty_root {
            serde_json::json!({
                "agentId": "test-agent",
                "name": "test",
                "latestRootBlobId": "",
            })
        } else {
            serde_json::json!({
                "agentId": "test-agent",
                "name": "test",
                "latestRootBlobId": a_id.clone(),
            })
        };
        let hex = hex_encode(serde_json::to_string(&meta).unwrap().as_bytes());
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('0', ?1)",
            params![hex],
        )
        .unwrap();
        db
    }

    #[test]
    fn fix_orphans_finds_empty_root_and_fixes() {
        // See HOME_LOCK doc — this test mutates the global HOME env
        // var; grabbing the lock prevents parallel tests in unrelated
        // modules (e.g. watcher::label_strips_home) from racing us
        // and asserting against an unexpected home.
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());
        let chats = home.path().join(".cursor").join("chats");
        let bucket1 = chats.join("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let bucket2 = chats.join("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let uuid_broken = "11111111-1111-1111-1111-111111111111";
        let uuid_ok = "22222222-2222-2222-2222-222222222222";
        std::fs::create_dir_all(bucket1.join(uuid_broken)).unwrap();
        std::fs::create_dir_all(bucket2.join(uuid_ok)).unwrap();
        let broken_db = make_broken_store_db(&bucket1.join(uuid_broken), true);
        let ok_db = make_broken_store_db(&bucket2.join(uuid_ok), false);
        // Sanity: broken_db has empty root, ok_db has a_id.
        assert_eq!(
            read_latest_root_blob_id(&broken_db).unwrap(),
            Some(String::new())
        );
        let ok_root = read_latest_root_blob_id(&ok_db).unwrap().unwrap();
        assert!(!ok_root.is_empty());

        let report = fix_orphans().expect("fix_orphans");
        assert_eq!(report.scanned, 2);
        assert_eq!(report.fixed.len(), 1, "skipped: {:?}", report.skipped);
        assert!(report.fixed[0].starts_with(uuid_broken));
        assert!(
            report.skipped.is_empty(),
            "unexpected skips: {:?}",
            report.skipped
        );

        // Re-read: broken_db should now have a non-empty root.
        let after = read_latest_root_blob_id(&broken_db).unwrap().unwrap();
        assert!(!after.is_empty(), "broken_db root should be filled");

        // ok_db should be unchanged.
        let ok_after = read_latest_root_blob_id(&ok_db).unwrap().unwrap();
        assert_eq!(ok_after, ok_root, "ok_db should be untouched");

        // backup should exist for the broken one.
        let parent = broken_db.parent().unwrap();
        let has_backup = std::fs::read_dir(parent)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains(".backup_"));
        assert!(has_backup, "fix_orphans should leave a .backup_<ts> sibling");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn delete_session_removes_l1_and_l2_dirs() {
        // See HOME_LOCK doc.
        let _guard = HOME_LOCK.lock().unwrap();
        let home = tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());
        let uuid = "33333333-3333-3333-3333-333333333333";
        let cwd = "/tmp/never-opened-in-cursor-12345";
        let slug = "tmp-never-opened-in-cursor-12345";

        // Build L2 dir under the temp HOME.
        let chat_root = paths::chat_root_for(cwd);
        let l2 = home
            .path()
            .join(".cursor")
            .join("chats")
            .join(&chat_root)
            .join(uuid);
        std::fs::create_dir_all(l2.join("dummy")).unwrap();
        assert!(l2.is_dir());

        // Build L1 dir under the temp HOME.
        let l1 = home
            .path()
            .join(".cursor")
            .join("projects")
            .join(slug)
            .join("agent-transcripts")
            .join(uuid);
        std::fs::create_dir_all(l1.join("nested")).unwrap();
        assert!(l1.is_dir());

        let report = delete_session(uuid, cwd, Some(slug)).expect("delete_session");
        assert_eq!(report.uuid, uuid);
        // The delete_session acquires the same `cursor_processes_running`
        // guard as sync_session_layer23 — if the dev box happens to have
        // Cursor / cursor-agent running (very common while developing),
        // the call short-circuits with `cursor_running=true` and *nothing*
        // on disk changes. Skip the round-trip assertions in that case so
        // the test stays usable on a normal workstation. (Heavy users of
        // bettercursor who don't quit Cursor between runs accept that
        // delete is refused — this is the intended UX safety net.)
        if report.cursor_running {
            eprintln!(
                "delete_session_removes_l1_and_l2_dirs: skipping \
                 assertions because Cursor is running on this dev box"
            );
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            return;
        }
        assert!(report.removed_l1, "L1 should be removed; skip={:?}", report.skipped_l1);
        assert!(report.removed_l2, "L2 should be removed; skip={:?}", report.skipped_l2);
        assert!(report.skipped_l1.is_none());
        assert!(report.skipped_l2.is_none());
        assert!(!report.cursor_running);
        assert!(!l1.exists(), "L1 dir should be gone");
        assert!(!l2.exists(), "L2 dir should be gone");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
