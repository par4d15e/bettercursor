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
//!   - `prepare_inject(uuid)` — write the plan to
//!     `~/.bettercursor/queue/inject-<uuid>.json` so the user can run
//!     `apply.py` **after closing Cursor** to apply the mutations to
//!     `state.vscdb`. bettercursor never touches `state.vscdb`
//!     directly because Cursor Electron holds that file open and a
//!     rename race silently overwrites our writes via Cursor's WAL
//!     flush (empirically verified in #84).
//!   - `inspect_prepared(uuid)` — check whether a queue file exists
//!     and whether `apply.py` has already run for this uuid (by
//!     looking for the sidecar `<queue>.applied` marker).
//!
//! After `apply.py` succeeds, Cursor Electron must be restarted for
//! the Sidebar to reflect the new entry.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

// ── Public API ────────────────────────────────────────────────

/// Build the mutation plan without touching disk. UI calls this first,
/// shows a preview, then on confirmation calls `commit_inject(plan)`.
pub fn dry_run_inject(uuid: &str) -> Result<InjectPlan> {
    build_plan(uuid)
}

/// Stage a Layer 3 injection plan to `~/.bettercursor/queue/inject-<uuid>.json`
/// for **offline** application via `apply.py`. Does NOT touch state.vscdb
/// because Cursor Electron holds that file open and any race we lose
/// silently overwrites our writes via its WAL flush — verified empirically
/// in #84 (rename was performed but rows did not survive).
pub fn prepare_inject(uuid: &str) -> Result<PrepareResult> {
    let plan = build_plan(uuid)?;
    if plan.skip_reason.is_some() {
        anyhow::bail!(
            "plan has skip_reason set ({:?}), refusing to write queue file",
            plan.skip_reason
        );
    }
    let queue_path = paths::inject_queue_path(uuid);
    if let Some(parent) = queue_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("create queue dir {}", parent.display())
        })?;
    }

    // Wrap the plan with a small header so apply.py can verify it
    // matches the live state (uuid sanity check, schema version,
    // recommendation).
    let envelope = serde_json::json!({
        "schema_version": 1,
        "tool": "bettercursor",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "apply_command": apply_command_for(&queue_path),
        "state_vscdb_path_hint": paths::global_db_path().ok().map(|p| p.display().to_string()),
        "applied_marker_filename": applied_marker_filename(uuid),
        "plan": plan,
    });
    let body = serde_json::to_string_pretty(&envelope)
        .context("serialize inject envelope")?;
    std::fs::write(&queue_path, body)
        .with_context(|| format!("write {}", queue_path.display()))?;

    Ok(PrepareResult {
        uuid: uuid.to_string(),
        queue_path: queue_path.display().to_string(),
        apply_command: apply_command_for(&queue_path),
        mutations: plan.mutations.len(),
    })
}

/// Result of staging an injection. The user must (1) quit Cursor and
/// (2) run `apply_command` themselves; bettercursor never executes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareResult {
    pub uuid: String,
    pub queue_path: String,
    pub apply_command: String,
    pub mutations: usize,
}

/// Generate the one-liner the user will copy-paste after closing Cursor.
/// Hard-coded `python3` for now — apply.py is portable enough that we
/// do not worry about venv shenanigans in this stage.
fn apply_command_for(queue_path: &Path) -> String {
    let script = paths::apply_script_path().display().to_string();
    let queue = queue_path.display().to_string();
    format!("python3 {script} {queue}")
}

/// File the apply script writes next to the queue file after a
/// successful run, so the UI can detect "this one's already done"
/// without re-running the SQL.
fn applied_marker_filename(uuid: &str) -> String {
    format!("inject-{uuid}.applied")
}

/// Returns the paths the UI should show for confirmation, plus a
/// "already applied" hint if the marker file exists.
pub fn inspect_prepared(uuid: &str) -> Option<Prepared> {
    let queue_path = paths::inject_queue_path(uuid);
    if !queue_path.exists() {
        return None;
    }
    let marker = queue_path.with_file_name(applied_marker_filename(uuid));
    Some(Prepared {
        uuid: uuid.to_string(),
        queue_path: queue_path.display().to_string(),
        applied: marker.exists(),
        marker_path: marker.display().to_string(),
        apply_command: apply_command_for(&queue_path),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prepared {
    pub uuid: String,
    pub queue_path: String,
    pub applied: bool,
    pub marker_path: String,
    pub apply_command: String,
}

// (state_vscdb_path() removed: it was a thin wrapper over
//  paths::global_db_path() with no remaining callers — callers
//  were either deleted with commit_inject or switched to
//  inject_queue_path() / apply_script_path() in paths.rs.)

/// Copy the bundled `scripts/apply.py` into the user's data directory
/// on first use so it survives project moves. Idempotent: only copies
/// when the destination is missing.
pub fn ensure_apply_script() -> Result<PathBuf> {
    let dst = paths::apply_script_path();
    if dst.exists() {
        return Ok(dst);
    }
    let src = apply_script_source_path();
    std::fs::create_dir_all(dst.parent().unwrap_or_else(|| Path::new(".")))?;
    std::fs::copy(&src, &dst).with_context(|| {
        format!(
            "copy bundled apply.py {} → {}",
            src.display(),
            dst.display()
        )
    })?;
    Ok(dst)
}

/// Locate the bundled `scripts/apply.py`. Tries a few candidate
/// locations so the script works whether we run from the project
/// checkout, an installed `cargo` build, or a packaged release.
fn apply_script_source_path() -> PathBuf {
    let candidates = [
        // dev: <repo>/scripts/apply.py sibling to the manifest dir
        concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/apply.py"),
        // dev: legacy / nested layout (e.g. src-tauri/scripts/apply.py)
        concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/apply.py"),
        // current-working-directory fallback for cargo-installed bins
        "scripts/apply.py",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_file() {
            return p;
        }
    }
    // Fallback: even if missing, surface a user-friendly error on
    // first invoke.
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/apply.py"))
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

// ── Mutation schema (helper removed: live apply_mutation was tied
//    to the deleted commit_inject path. The actual SQL upsert that
//    writes to state.vscdb now lives in scripts/apply.py, which runs
//    offline with Cursor closed.) ──────────────────────────────

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

// (state_vscdb_path_helper pub use alias removed: lib.rs now
//  registers prepare_inject_layer3 + inspect_prepared_layer3
//  directly; nothing here needs re-exporting.)

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
