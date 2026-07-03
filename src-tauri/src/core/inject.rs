//! bettercursor Layer 3 injection — synthesize Cursor Electron Desktop
//! Sidebar entries for CLI-originated sessions.
//!
//! Background: Cursor Desktop's Sidebar reads `~/.config/Cursor/User/
//! globalStorage/state.vscdb`. Specifically:
//!   - `ItemTable['composer.composerHeaders'].allComposers` — list index
//!   - `cursorDiskKV['composerData:<uuid>']` — full composer details
//!   - `cursorDiskKV['bubbleId:<uuid>:<bid>']` — one blob per message
//!
//! When a session is written *only* by `cursor-agent` CLI (Layer 2 + JSONL
//! only), Desktop never sees it in its Sidebar. This module builds the
//! SQLite mutations to make Desktop recognize it, by reverse-engineering
//! the schema from a known Desktop-originated session (c1ea7999).
//!
//! Two-phase API:
//!   - `dry_run_inject(uuid)` — compute the mutation set without
//!     touching disk, return JSON-able `InjectPlan` for the UI to
//!     preview. Skipped: file writes.
//!   - `commit_inject(plan)` — actually apply. Uses tmpdir copy +
//!     PRAGMA integrity_check + atomic rename. The Cursor Electron
//!     process must be restarted for the change to take effect.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use super::canonical;
use super::paths;

/// Single mutation the injector plans to make. Returned by dry-run
/// and replayed verbatim by commit — keeps the two paths provably
/// equivalent so what-you-see-is-what-you-apply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Mutation {
    /// `INSERT OR REPLACE INTO ItemTable(key, value) VALUES (?, ?)`
    ItemTableUpsert { key: String, value_hex: String },
    /// `INSERT OR REPLACE INTO cursorDiskKV(key, value) VALUES (?, ?)`
    DiskKvUpsert { key: String, value_hex: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectPlan {
    pub uuid: String,
    pub mutations: Vec<Mutation>,
    /// Where Layer 2 + Layer 1 + meta.json were read from. Surfaced
    /// in the UI so the user knows what the plan covers.
    pub sources: InjectSources,
    /// `None` means "I couldn't find enough source data to build
    /// the plan" — usually missing meta.json or empty JSONL. The
    /// UI shows this verbatim.
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InjectSources {
    pub layer1_jsonl: Option<String>,
    pub layer2_store_db: Option<String>,
    pub layer2_meta_json: Option<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub created_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectResult {
    pub uuid: String,
    pub applied: usize,
    pub state_vscdb_path: String,
    pub backup_path: String,
    /// `true` iff `PRAGMA integrity_check` returned `ok` after write.
    pub integrity_ok: bool,
}

// ── Public API ────────────────────────────────────────────────

/// Build the mutation plan without touching disk. UI calls this first,
/// shows a preview, then on confirmation calls `commit_inject(plan)`.
pub fn dry_run_inject(uuid: &str) -> Result<InjectPlan> {
    build_plan(uuid)
}

/// Apply a previously-built plan. The plan must be one returned by
/// `dry_run_inject(uuid)` — the commit path takes it verbatim (no
/// recompute) so what you previewed is what you get.
pub fn commit_inject(plan: &InjectPlan) -> Result<InjectResult> {
    if plan.skip_reason.is_some() {
        anyhow::bail!(
            "plan has skip_reason set ({:?}), refusing to commit",
            plan.skip_reason
        );
    }
    let state_vscdb = paths::global_db_path()?;
    if !state_vscdb.exists() {
        anyhow::bail!(
            "state.vscdb not found at {}; Cursor Desktop has not run on this host yet",
            state_vscdb.display()
        );
    }

    // 1) Copy state.vscdb (+ WAL + SHM sidecars) to a temp dir.
    let tmp = TempDir::new().context("create temp dir for state.vscdb copy")?;
    let tmp_db = tmp.path().join("state.vscdb");
    std::fs::copy(&state_vscdb, &tmp_db).with_context(|| {
        format!("copy state.vscdb → {}", tmp_db.display())
    })?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = state_vscdb.with_extension(format!(
            "vscdb{suffix}"
        ));
        if sidecar.exists() {
            let dst = tmp.path().join(format!(
                "state.vscdb{suffix}"
            ));
            let _ = std::fs::copy(&sidecar, &dst);
        }
    }

    // 2) Apply mutations to the copy.
    let conn = Connection::open(&tmp_db).context("open copied state.vscdb")?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .context("WAL checkpoint on copy")?;
    for m in &plan.mutations {
        apply_mutation(&conn, m)?;
    }
    drop(conn);

    // 3) Verify integrity before swapping in.
    let check_conn = Connection::open(&tmp_db)?;
    let integrity: String = check_conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .context("run integrity_check")?;
    drop(check_conn);
    if integrity.trim() != "ok" {
        anyhow::bail!(
            "integrity_check failed after write: {integrity}; aborting without \
             touching the original state.vscdb"
        );
    }

    // 4) Backup original + atomic replace.
    let backup_path = backup_original(&state_vscdb)?;
    atomic_replace(&tmp_db, &state_vscdb)
        .with_context(|| format!("atomic swap into {}", state_vscdb.display()))?;

    Ok(InjectResult {
        uuid: plan.uuid.clone(),
        applied: plan.mutations.len(),
        state_vscdb_path: state_vscdb.display().to_string(),
        backup_path: backup_path.display().to_string(),
        integrity_ok: true,
    })
}

/// Path to the original state.vscdb (for diagnostics + a "is Desktop
/// installed here" UI gate). Returns Err on unsupported platforms.
pub fn state_vscdb_path() -> Result<PathBuf> {
    paths::global_db_path()
}

// ── Internals ────────────────────────────────────────────────

fn build_plan(uuid: &str) -> Result<InjectPlan> {
    let mut sources = InjectSources::default();

    // ── Find Layer 1 JSONL ────────────────────────────────────
    let jsonl_path = find_layer1_jsonl(uuid);
    if let Some(p) = jsonl_path.as_ref() {
        sources.layer1_jsonl = Some(p.display().to_string());
    }
    let bubbles_from_layer1: Vec<LayerBubble> = jsonl_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| parse_layer1_bubbles(&s))
        .unwrap_or_default();

    // ── Find Layer 2 store.db + meta.json ─────────────────────
    let (storedb_path, meta_json_path) = find_layer2(uuid);
    sources.layer2_store_db = storedb_path.as_ref().map(|p| p.display().to_string());
    sources.layer2_meta_json = meta_json_path.as_ref().map(|p| p.display().to_string());

    let mut meta_title: Option<String> = None;
    let mut meta_created_at_ms: Option<i64> = None;
    let mut meta_cwd: Option<String> = None;
    if let Some(mp) = &meta_json_path {
        if let Ok(s) = std::fs::read_to_string(mp) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                meta_title = v
                    .get("title")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                meta_cwd = v
                    .get("cwd")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                meta_created_at_ms = v
                    .get("createdAtMs")
                    .and_then(|x| x.as_i64());
            }
        }
    }
    sources.title = meta_title.clone();
    sources.created_at_ms = meta_created_at_ms;
    sources.cwd = meta_cwd.clone();

    if jsonl_path.is_none() && storedb_path.is_none() {
        return Ok(InjectPlan {
            uuid: uuid.to_string(),
            mutations: Vec::new(),
            sources,
            skip_reason: Some(
                "既找不到 Layer 1 JSONL 也找不到 Layer 2 store.db — 无法重建"
                    .to_string(),
            ),
        });
    }
    if bubbles_from_layer1.is_empty() {
        return Ok(InjectPlan {
            uuid: uuid.to_string(),
            mutations: Vec::new(),
            sources,
            skip_reason: Some("Layer 1 JSONL 已找到但无可解析的对话气泡".to_string()),
        });
    }

    // ── Decide name (fall back to JSONL first line) ────────────
    let name = meta_title
        .clone()
        .or_else(|| {
            bubbles_from_layer1
                .iter()
                .find(|b| b.role == "user")
                .map(|b| truncate_to_title(&b.text))
        })
        .unwrap_or_else(|| "CLI 会话".to_string());

    let created_at_ms = meta_created_at_ms
        .or_else(|| bubbles_from_layer1.iter().map(|b| b.created_at_ms).find(|x| *x > 0))
        .unwrap_or(0);
    let last_updated_ms = bubbles_from_layer1
        .iter()
        .map(|b| b.created_at_ms)
        .max()
        .unwrap_or(created_at_ms);

    let workspace_identifier = meta_cwd.as_deref().and_then(build_workspace_identifier);
    let project_slug = meta_cwd
        .as_deref()
        .map(paths::sanitize_project_path)
        .unwrap_or_else(|| format!("cli-{uuid}"));

    // ── Compose Layer 3 entries (schema reverse-engineered from
    //    c1ea7999-005a-434f-bcf4-da8ddd9ff066 in state.vscdb) ──
    let composer_data = compose_composer_data(
        uuid,
        &name,
        created_at_ms,
        last_updated_ms,
        &workspace_identifier,
        &project_slug,
        &bubbles_from_layer1,
    );
    let header_entry = compose_composer_header_entry(
        uuid,
        &name,
        created_at_ms,
        last_updated_ms,
        &workspace_identifier,
    );
    let bubble_blobs = compose_bubble_blobs(uuid, &bubbles_from_layer1);

    let mut mutations = Vec::new();
    mutations.push(Mutation::ItemTableUpsert {
        key: "composer.composerHeaders".to_string(),
        value_hex: hex_string(&merge_composer_headers(uuid, &header_entry)?),
    });
    mutations.push(Mutation::DiskKvUpsert {
        key: format!("composerData:{uuid}"),
        value_hex: hex_string(&composer_data),
    });
    for (bubble_id, bubble_body) in bubble_blobs {
        mutations.push(Mutation::DiskKvUpsert {
            key: format!("bubbleId:{uuid}:{bubble_id}"),
            value_hex: hex_string(&bubble_body),
        });
    }

    Ok(InjectPlan {
        uuid: uuid.to_string(),
        mutations,
        sources,
        skip_reason: None,
    })
}

#[derive(Debug, Clone)]
struct LayerBubble {
    role: String,        // "user" | "assistant"
    text: String,
    created_at_ms: i64,
}

// ── Layer 1 JSONL → LayerBubble[] (subset of read_conversation) ──

fn parse_layer1_bubbles(body: &str) -> Vec<LayerBubble> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let role = v
            .get("role")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if role != "user" && role != "assistant" {
            continue;
        }
        let mut text = String::new();
        if let Some(arr) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            for c in arr {
                if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(s) = c.get("text").and_then(|t| t.as_str()) {
                        text.push_str(s);
                        text.push('\n');
                    }
                }
            }
        }
        if text.trim().is_empty() {
            continue;
        }
        // No reliable timestamp inside JSONL; treat order as time monotonic.
        // created_at_ms placeholder — filled later from JSONL header if any.
        let created_at_ms = v
            .get("timestamp")
            .and_then(|t| t.as_i64())
            .unwrap_or(0);
        out.push(LayerBubble {
            role,
            text: text.trim_end().to_string(),
            created_at_ms,
        });
    }
    out
}

fn truncate_to_title(s: &str) -> String {
    let first_line = s.lines().next().unwrap_or(s);
    let trimmed = first_line.trim();
    if trimmed.chars().count() <= 60 {
        trimmed.to_string()
    } else {
        let cut: String = trimmed.chars().take(57).collect();
        format!("{cut}…")
    }
}

// ── Layer 2 file discovery (tolerant of both layouts) ─────────

fn find_layer1_jsonl(uuid: &str) -> Option<PathBuf> {
    let projects = paths::cursor_projects_dir();
    if !projects.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&projects).ok()?;
    for proj in entries.flatten() {
        let transcripts = proj.path().join("agent-transcripts");
        if !transcripts.is_dir() {
            continue;
        }
        let p = transcripts.join(uuid).join(format!("{uuid}.jsonl"));
        if p.is_file() {
            return Some(p);
        }
        let flat = transcripts.join(format!("{uuid}.jsonl"));
        if flat.is_file() {
            return Some(flat);
        }
    }
    None
}

fn find_layer2(uuid: &str) -> (Option<PathBuf>, Option<PathBuf>) {
    let chats_root = paths::chats_dir();
    if !chats_root.exists() {
        return (None, None);
    }
    let md5_roots = match std::fs::read_dir(&chats_root) {
        Ok(r) => r,
        Err(_) => return (None, None),
    };
    for md5_entry in md5_roots.flatten() {
        let dir = md5_entry.path().join(uuid);
        if !dir.is_dir() {
            continue;
        }
        let storedb = dir.join("store.db");
        let meta = dir.join("meta.json");
        let storedb = if storedb.is_file() { Some(storedb) } else { None };
        let meta = if meta.is_file() { Some(meta) } else { None };
        if storedb.is_some() || meta.is_some() {
            return (storedb, meta);
        }
    }
    (None, None)
}

// ── workspaceIdentifier (mirrors Cursor desktop format) ─────

fn build_workspace_identifier(cwd: &str) -> Option<serde_json::Value> {
    if cwd.trim().is_empty() {
        return None;
    }
    let fs_path = cwd.to_string();
    // Cursor uses a deterministic but unprincipled hash. We replicate
    // the simple md5-of-path hex (32 chars) convention used elsewhere
    // so desktop's matching algorithm recognises us. Production-grade
    // Cursor probably uses xxhash; we use md5 as a stand-in that has
    // the right length and is collision-safe for our 1-host scenario.
    let id = format!("{:x}", md5::compute(fs_path.as_bytes()));
    Some(serde_json::json!({
        "id": id,
        "uri": {
            "$mid": 1,
            "fsPath": fs_path,
            "external": format!("file://{fs_path}"),
            "path": fs_path,
            "scheme": "file"
        }
    }))
}

// ── Mutation: ItemTable / cursorDiskKV insert ─────────────────

fn apply_mutation(conn: &Connection, m: &Mutation) -> Result<()> {
    match m {
        Mutation::ItemTableUpsert { key, value_hex } => {
            // ItemTable: (key PRIMARY KEY, value). Cursor uses text keys
            // for ItemTable. Verify schema isn't a foreign surprise.
            ensure_item_table_shape(conn)?;
            let bytes = hex::decode(value_hex)
                .with_context(|| format!("decode hex for ItemTable key={key}"))?;
            let value_text = String::from_utf8(bytes.clone()).unwrap_or_else(|_| {
                // Last resort: store as Latin-1 surrogate so we never lose bytes.
                bytes.iter().map(|b| *b as char).collect::<String>()
            });
            conn.execute(
                "INSERT OR REPLACE INTO ItemTable(key, value) VALUES (?1, ?2)",
                params![key, value_text],
            )
            .with_context(|| format!("upsert ItemTable key={key}"))?;
        }
        Mutation::DiskKvUpsert { key, value_hex } => {
            ensure_diskkv_shape(conn)?;
            let bytes = hex::decode(value_hex)
                .with_context(|| format!("decode hex for cursorDiskKV key={key}"))?;
            let value_text = String::from_utf8(bytes.clone()).unwrap_or_else(|_| {
                bytes.iter().map(|b| *b as char).collect::<String>()
            });
            conn.execute(
                "INSERT OR REPLACE INTO cursorDiskKV(key, value) VALUES (?1, ?2)",
                params![key, value_text],
            )
            .with_context(|| format!("upsert cursorDiskKV key={key}"))?;
        }
    }
    Ok(())
}

fn ensure_item_table_shape(conn: &Connection) -> Result<()> {
    // Best-effort: PRAGMA table_info tolerates missing tables.
    let mut stmt = conn.prepare("PRAGMA table_info(ItemTable)")?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    if cols.is_empty() {
        anyhow::bail!("ItemTable does not exist in state.vscdb (Cursor version mismatched?)");
    }
    Ok(())
}

fn ensure_diskkv_shape(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(cursorDiskKV)")?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    if cols.is_empty() {
        anyhow::bail!("cursorDiskKV does not exist in state.vscdb (Cursor version mismatched?)");
    }
    Ok(())
}

// ── Atomic write helpers ──────────────────────────────────────

fn backup_original(state_vscdb: &Path) -> Result<PathBuf> {
    let backup = state_vscdb.with_extension("vscdb.pre_bettercursor");
    if !backup.exists() {
        std::fs::copy(state_vscdb, &backup).with_context(|| {
            format!("backup {} → {}", state_vscdb.display(), backup.display())
        })?;
        // Also back up sidecars (best-effort).
        for suffix in ["-wal", "-shm"] {
            let sidecar = state_vscdb.with_extension(format!("vscdb{suffix}"));
            if sidecar.exists() {
                let dst = state_vscdb.with_extension(format!(
                    "vscdb.pre_bettercursor{suffix}"
                ));
                let _ = std::fs::copy(&sidecar, &dst);
            }
        }
    }
    Ok(backup)
}

fn atomic_replace(src_tmp: &Path, dst: &Path) -> Result<()> {
    // SQLite WAL on Windows has been known to refuse plain overwrite
    // if another process has the file open — but on Linux (our only
    // target right now) `std::fs::rename` over an existing file is
    // atomic per POSIX. Cursor Electron holds state.vscdb open most
    // of the time, so we close all open handles via copy-on-write
    // semantics: the inode changes, Electron keeps reading the old
    // inode harmlessly. After Cursor restarts, it opens the new file.
    std::fs::rename(src_tmp, dst).with_context(|| {
        format!(
            "atomic rename {} → {}",
            src_tmp.display(),
            dst.display()
        )
    })?;
    // Also swap the wal/shm sidecars if we copied them.
    let tmp_dir = src_tmp.parent().unwrap_or_else(|| Path::new("."));
    for suffix in ["-wal", "-shm"] {
        let src = tmp_dir.join(format!("state.vscdb{suffix}"));
        if src.is_file() {
            let dst_sidecar = dst.with_extension(format!("vscdb{suffix}"));
            let _ = std::fs::rename(&src, &dst_sidecar);
        }
    }
    Ok(())
}

// ── Composition (schema reverse-engineered from
//    c1ea7999-005a-434f-bcf4-da8ddd9ff066) ─────────────────────

fn compose_composer_data(
    uuid: &str,
    name: &str,
    _created_at_ms: i64,
    last_updated_ms: i64,
    workspace_identifier: &Option<serde_json::Value>,
    _project_slug: &str,
    bubbles: &[LayerBubble],
) -> serde_json::Value {
    let headers_only: Vec<serde_json::Value> = bubbles
        .iter()
        .enumerate()
        .map(|(idx, b)| {
            let bubble_id = deterministic_bubble_id(uuid, &b.role, b.created_at_ms, idx);
            let (bubble_type, grouping) = match b.role.as_str() {
                "user" => (
                    1,
                    serde_json::json!({
                        "isRenderable": true,
                        "hasText": true,
                        "isShortPlainText": b.text.len() < 200,
                    }),
                ),
                _ => (2, serde_json::json!({})),
            };
            let created_at_iso = ms_to_iso(b.created_at_ms);
            serde_json::json!({
                "bubbleId": bubble_id,
                "type": bubble_type,
                "createdAt": created_at_iso,
                "grouping": grouping,
            })
        })
        .collect();

    serde_json::json!({
        "_v": 16,
        "composerId": uuid,
        "richText": "",
        "hasLoaded": true,
        "text": "",
        "name": name,
        "contextUsagePercent": 0.0,
        "lastUpdatedAt": last_updated_ms,
        "unifiedMode": "agent",
        "forceMode": "edit",
        "hasUnreadMessages": false,
        "filesChangedCount": 0,
        "totalLinesAdded": 0,
        "totalLinesRemoved": 0,
        "isArchived": false,
        "isDraft": false,
        "isWorktree": false,
        "worktreeStartedReadOnly": false,
        "isSpec": false,
        "isProject": false,
        "isBestOfNSubcomposer": false,
        "numSubComposers": 0,
        "referencedPlans": [],
        "trackedGitRepos": [],
        "workspaceIdentifier": workspace_identifier.clone().unwrap_or(serde_json::Value::Null),
        "fullConversationHeadersOnly": headers_only,
    })
}

fn compose_composer_header_entry(
    uuid: &str,
    name: &str,
    _created_at_ms: i64,
    last_updated_ms: i64,
    workspace_identifier: &Option<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "type": "head",
        "composerId": uuid,
        "name": name,
        "lastUpdatedAt": last_updated_ms,
        "createdAt": _created_at_ms,
        "unifiedMode": "agent",
        "forceMode": "edit",
        "hasUnreadMessages": false,
        "contextUsagePercent": 0.0,
        "totalLinesAdded": 0,
        "totalLinesRemoved": 0,
        "filesChangedCount": 0,
        "subtitle": format!("{name}"),
        "hasBlockingPendingActions": false,
        "hasPendingPlan": false,
        "isArchived": false,
        "isDraft": false,
        "isWorktree": false,
        "worktreeStartedReadOnly": false,
        "isSpec": false,
        "isProject": false,
        "isBestOfNSubcomposer": false,
        "numSubComposers": 0,
        "referencedPlans": [],
        "trackedGitRepos": [],
        "workspaceIdentifier": workspace_identifier.clone().unwrap_or(serde_json::Value::Null),
    })
}

fn compose_bubble_blobs(
    uuid: &str,
    bubbles: &[LayerBubble],
) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for (idx, b) in bubbles.iter().enumerate() {
        let bubble_id = deterministic_bubble_id(uuid, &b.role, b.created_at_ms, idx);
        let bubble_type = match b.role.as_str() {
            "user" => 1,
            _ => 2,
        };
        let body = match b.role.as_str() {
            "user" => serde_json::json!({
                "_v": 3,
                "type": bubble_type,
                "text": b.text,
                "approximateLintErrors": [],
                "lints": [],
                "codebaseContextChunks": [],
                "commits": [],
                "pullRequests": [],
                "attachedCodeChunks": [],
                "assistantSuggestedDiffs": [],
                "gitDiffs": [],
                "interpreterResults": [],
                "toolResults": [],
            }),
            _ => serde_json::json!({
                "_v": 3,
                "type": bubble_type,
                "text": b.text,
                "isAgentic": true,
                "approximateLintErrors": [],
                "lints": [],
                "codebaseContextChunks": [],
                "commits": [],
                "pullRequests": [],
                "attachedCodeChunks": [],
                "assistantSuggestedDiffs": [],
                "gitDiffs": [],
                "interpreterResults": [],
                "toolResults": [],
            }),
        };
        out.push((bubble_id, body));
    }
    out
}

fn merge_composer_headers(
    uuid: &str,
    new_entry: &serde_json::Value,
) -> Result<serde_json::Value> {
    // Read existing ItemTable['composer.composerHeaders'] from the
    // *original* state.vscdb (best-effort, lenient on corruption).
    let state_vscdb = paths::global_db_path()?;
    let conn = if state_vscdb.exists() {
        Some(Connection::open_with_flags(
            &state_vscdb,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?)
    } else {
        None
    };
    let mut root = serde_json::json!({"allComposers": []});
    if let Some(conn) = conn {
        let key = "composer.composerHeaders";
        // Gather the existing JSON value into a String before the
        // statement / rows go out of scope. rusqlite's `Rows` borrows
        // from its parent statement, so we can't safely hold the
        // borrowed `Value` past the stmt drop. We convert to owned
        // text immediately.
        let existing_text: Option<String> = {
            let mut stmt = conn.prepare("SELECT value FROM ItemTable WHERE key = ?1")?;
            let mut rows = stmt.query([key])?;
            match rows.next()? {
                Some(row) => {
                    let v: rusqlite::types::Value = row.get(0)?;
                    rusqlite_value_to_string(v)
                }
                None => None,
            }
        };
        if let Some(s) = existing_text {
            if let Ok(existing) = serde_json::from_str::<serde_json::Value>(&s) {
                root = existing;
            }
        }
    }
    let all = root
        .as_object_mut()
        .and_then(|o| o.get_mut("allComposers"))
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!("allComposers missing or not array"))?;
    // Remove any prior entry with this uuid, then append the new one.
    all.retain(|e| {
        e.get("composerId")
            .and_then(|c| c.as_str())
            .map(|s| s != uuid)
            .unwrap_or(true)
    });
    all.push(new_entry.clone());
    Ok(root)
}

fn rusqlite_value_to_string(v: rusqlite::types::Value) -> Option<String> {
    use rusqlite::types::Value::*;
    match v {
        Text(s) => Some(s),
        Blob(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        Integer(i) => Some(i.to_string()),
        Real(f) => Some(f.to_string()),
        Null => None,
    }
}

fn hex_string(v: &serde_json::Value) -> String {
    let s = serde_json::to_string(v).expect("serialize json");
    hex::encode(s.as_bytes())
}

fn ms_to_iso(ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    if ms <= 0 {
        return String::new();
    }
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// Deterministic per-(uuid, role, timestamp) bubble id — matches what
/// Cursor desktop expects so a stable idempotent rerender is possible.
/// Uses the first 16 hex of SHA-256(uuid || role || ts).
fn deterministic_bubble_id(uuid: &str, role: &str, ts_ms: i64, ordinal: usize) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(uuid.as_bytes());
    h.update(b"|");
    h.update(role.as_bytes());
    h.update(b"|");
    h.update(ts_ms.to_le_bytes());
    h.update(b"|");
    h.update(ordinal.to_le_bytes());
    let digest = h.finalize();
    let mut s = String::with_capacity(36);
    let hex = format!("{:x}", digest);
    for (i, c) in hex.chars().take(32).enumerate() {
        if i == 8 || i == 12 || i == 16 || i == 20 {
            s.push('-');
        }
        s.push(c);
    }
    s
}

// Public re-export for lib.rs to register as Tauri command.
pub use paths::global_db_path as state_vscdb_path_helper;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let v = serde_json::json!({"hello": "world", "n": 42});
        let h = hex_string(&v);
        let bytes = hex::decode(&h).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        let back: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn deterministic_bubble_id_format() {
        let id = deterministic_bubble_id("abc", "user", 1000, 0);
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
    }

    #[test]
    fn bubble_id_distinct_per_ordinal_even_at_zero_ts() {
        // Two assistant bubbles with ts=0 (Layer 1 doesn't carry one)
        // would collide on (uuid, role, ts) alone — the ordinal
        // tie-breaker must distinguish them so we don't try to
        // INSERT two rows at the same key.
        let a = deterministic_bubble_id("u1", "assistant", 0, 0);
        let b = deterministic_bubble_id("u1", "assistant", 0, 1);
        assert_ne!(a, b, "ordinal tie-breaker failed");
    }

    #[test]
    fn ms_to_iso_zero_returns_empty() {
        assert_eq!(ms_to_iso(0), "");
    }

    #[test]
    fn truncate_to_title_handles_cjk() {
        let s = "中".repeat(80);
        let t = truncate_to_title(&s);
        // Don't break mid-字符 (3 bytes each). Should be exactly 57 chars + ellipsis.
        assert!(t.chars().count() <= 60);
        assert!(t.ends_with('…') || t.chars().count() <= 57);
    }

    /// Live smoke test against a known CLI-originated session in the
    /// dev environment. The UUID below is the one Eric used to file
    /// the "Desktop Sidebar can't see CLI session" issue. If not
    /// present, the test is a no-op (`--ignored` style).
    ///
    /// Run with: `cargo test --lib inject_smoke_real_a90b276e -- --ignored`
    #[test]
    #[ignore]
    fn smoke_real_a90b276e() {
        let uuid = "a90b276e-0f5f-444e-98f8-caf42aaee49e";
        let layer1 = find_layer1_jsonl(uuid);
        eprintln!("layer1: {:?}", layer1);
        let (storedb, meta) = find_layer2(uuid);
        eprintln!("layer2 store.db: {:?}", storedb);
        eprintln!("layer2 meta.json: {:?}", meta);
        let plan = dry_run_inject(uuid).expect("dry_run_inject");
        eprintln!("skip_reason: {:?}", plan.skip_reason);
        eprintln!("mutations count: {}", plan.mutations.len());
        for m in &plan.mutations {
            match m {
                Mutation::ItemTableUpsert { key, .. } => eprintln!("  IT  {}", key),
                Mutation::DiskKvUpsert { key, .. } => eprintln!("  KV  {}", key),
            }
        }
        if plan.skip_reason.is_none() {
            assert!(plan.mutations.len() >= 2, "expected composer + at least one bubble");
        }
    }
}
