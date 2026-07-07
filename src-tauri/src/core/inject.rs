//! bettercursor Layer 3 schema composition — synthesize the SQLite
//! rows Cursor Electron's Sidebar reads for CLI-originated sessions.
//!
//! Background
//! ----------
//! Cursor Desktop's Sidebar reads `~/.config/Cursor/User/globalStorage/
//! state.vscdb`. Specifically:
//!   - `ItemTable['composer.composerHeaders'].allComposers` — list index
//!   - `cursorDiskKV['composerData:<uuid>']` — full composer details
//!   - `cursorDiskKV['bubbleId:<uuid>:<bid>']` — one blob per message
//!
//! `core::sync` writes these rows inline via the `Mutation` type
//! defined here. This module is **schema-only**: it composes JSON
//! values that match what a real Desktop-originated session looks
//! like in state.vscdb (reverse-engineered from the c1ea7999
//! snapshot). It does not touch disk itself.
//!
//! Public surface (consumed by `core::sync::write_layer3`):
//!   - [`Mutation`] enum + [`Mutation::value_hex`] accessor — the
//!     upsert descriptor that `apply_mutation_inline` translates into
//!     SQL.
//!   - [`LayerBubble`] — parsed bubble from Layer 1 JSONL.
//!   - [`parse_layer1_bubbles`] — JSONL → `Vec<LayerBubble>`.
//!   - [`truncate_to_title_pub`] — first-line cut with CJK-safe width.
//!   - [`find_layer1_jsonl`] — locate the Layer 1 transcript file.
//!   - [`build_workspace_identifier`] — `workspaceIdentifier` JSON,
//!     preferring Cursor's `workspaceStorage` hash over MD5(cwd).
//!   - [`scan_tracked_git_repos`] — minimal `{repoPath, branches}`
//!     array Cursor writes for git workspaces.
//!   - [`compose_composer_data`] / [`compose_composer_header_entry`]
//!     / [`compose_bubble_blobs`] — three pieces of state.vscdb JSON.
//!   - [`merge_composer_headers`] — read existing `allComposers` from
//!     state.vscdb and append/replace our entry.
//!   - [`archive_composer_sidebar_entry`] — Desktop-equivalent soft
//!     delete: `isArchived: true`, clear `name` / `subtitle`.
//!
//! Schema reverse-engineered from
//! `c1ea7999-005a-434f-bcf4-da8ddd9ff066` in state.vscdb.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use super::paths;

/// Single mutation the sync writer plans to make. Written verbatim
/// by `core::sync::apply_mutation_inline` — keeps schema (this file)
/// and SQL application (sync.rs) decoupled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Mutation {
    /// `INSERT OR REPLACE INTO ItemTable(key, value) VALUES (?, ?)`
    ItemTableUpsert { key: String, value_hex: String },
    /// `INSERT OR REPLACE INTO cursorDiskKV(key, value) VALUES (?, ?)`
    DiskKvUpsert { key: String, value_hex: String },
}

impl Mutation {
    /// Hex-encoded value bytes — what gets written verbatim to the
    /// `value` column.
    pub fn value_hex(&self) -> &str {
        match self {
            Mutation::ItemTableUpsert { value_hex, .. } => value_hex,
            Mutation::DiskKvUpsert { value_hex, .. } => value_hex,
        }
    }
}

/// v0.2.2: LayerBubble is now a type alias for [`canonical::Bubble`].
/// Previously a parallel struct with 3 fields (`role`/`text`/`created_at_ms`)
/// — diverging from `canonical::Bubble` made 3-layer reconciliation
/// impossible. The alias preserves source compatibility for every
/// existing call site that takes `&[LayerBubble]` while funneling all
/// bubble construction through one canonical type.
pub type LayerBubble = super::canonical::Bubble;

// ── Layer 1 JSONL → Bubble[] (subset of read_conversation) ──

/// Parse a raw JSONL body into [`canonical::Bubble`] values.
///
/// v0.2.2: takes `uuid` (needed to fill each bubble's stable id via
/// [`deterministic_bubble_id`]). Callers without a uuid can pass `""` —
/// the id will then collide on `role|ts|ordinal` for that empty uuid,
/// which is OK for the L2/L3 synthesis path (the write path doesn't
/// rely on bubble IDs, only on text/role/ts).
///
/// v0.2.2: each bubble's `id` is set to
/// `deterministic_bubble_id(uuid, role, created_at_ms, ordinal)` and
/// `created_at_ms` is the line's `timestamp` field (0 when missing,
/// which is the common case for cursor-agent JSONL).
pub fn parse_layer1_bubbles(uuid: &str, body: &str) -> Vec<super::canonical::Bubble> {
    let mut out = Vec::new();
    for (ordinal, line) in body.lines().enumerate() {
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
        let created_at_ms = v.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
        let trimmed_text = text.trim_end().to_string();
        let id = deterministic_bubble_id(uuid, &role, created_at_ms, ordinal);
        out.push(super::canonical::Bubble {
            id,
            role,
            text: trimmed_text,
            tool_calls: Vec::new(),
            files: Vec::new(),
            images: Vec::new(),
            created_at_ms,
            parent_bubble_id: None,
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

/// Exposed for `core::sync` (Layer 3 synthesis from Layer 1 bubbles).
pub fn truncate_to_title_pub(s: &str) -> String {
    truncate_to_title(s)
}

// ── Layer 1 file discovery (consolidated in `paths::find_layer1_jsonl_for`) ──
//
// As of v0.2.2 the previously-duplicated JSONL finder lives in
// `super::paths::find_layer1_jsonl_for`. The old `inject::find_layer1_jsonl`
// and `canonical::find_jsonl_for` have both been removed; callers in
// `canonical::read_conversation` and `sync::read_layer1` were migrated.

// ── workspaceIdentifier (mirrors Cursor desktop format) ─────
//
// Cursor's Desktop Sidebar uses the *workspaceStorage directory hash*
// (e.g. `b9c96f3499915796f28905f2e97f8164` for /home/eric/workspace/
// bettercursor) as `workspaceIdentifier.id`, NOT MD5(cwd). The
// directory name is whatever Cursor's window state code chose when
// the user first opened the folder — empirically an xxhash /
// something-internal 32-char hex. We can't reverse-engineer that
// hash from the path alone; instead we read each candidate's
// `workspace.json` and match the `folder` URI to our cwd. If the
// folder has never been opened in Cursor, we fall back to MD5(cwd)
// (matches Layer 2's `chat_root_for`, which is what we used to ship,
// and at least produces a syntactically-valid 32-char hex id — the
// Sidebar will just not link the entry back to a known workspace).

pub fn build_workspace_identifier(cwd: &str) -> Option<serde_json::Value> {
    if cwd.trim().is_empty() {
        return None;
    }
    let fs_path = cwd.to_string();
    let id = resolve_workspace_storage_id(&fs_path)
        .unwrap_or_else(|| format!("{:x}", md5::compute(fs_path.as_bytes())));
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

/// Scan `~/.config/Cursor/User/workspaceStorage/<dir>/workspace.json`
/// and return the directory hash whose `folder` matches `cwd`. The
/// `folder` field is normally a `file://` URI; we tolerate a bare
/// path too. Returns None if the dir is missing or no entry matches —
/// callers fall back to MD5(cwd) in that case.
fn resolve_workspace_storage_id(cwd: &str) -> Option<String> {
    let dir = paths::workspace_storage_dir().ok()?;
    if !dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let ws_json = entry.path().join("workspace.json");
        if !ws_json.is_file() {
            continue;
        }
        let body = match std::fs::read_to_string(&ws_json) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let folder = v.get("folder").and_then(|x| x.as_str()).unwrap_or("");
        // Cursor writes `file:///abs/path` (note triple-slash). We
        // strip the scheme prefix and compare against cwd verbatim.
        let folder_path = folder
            .strip_prefix("file://")
            .unwrap_or(folder)
            .trim_end_matches('/');
        let cwd_trim = cwd.trim_end_matches('/');
        if folder_path == cwd_trim {
            let hash = entry.file_name().to_string_lossy().into_owned();
            if !hash.is_empty() {
                return Some(hash);
            }
        }
    }
    None
}

/// Walk `cwd` looking for git repos Cursor would track. Returns
/// an array of `{repoPath, branches:[{branchName}]}` ready to drop
/// into `trackedGitRepos`. Best-effort: only the current branch
/// name (from `.git/HEAD`) is recorded; upstream/last-interaction
/// are omitted. If no .git is found in the cwd's tree, returns an
/// empty vec — that's fine, real entries do the same for sessions
/// opened in non-git workspaces.
pub fn scan_tracked_git_repos(cwd: &str) -> Vec<serde_json::Value> {
    let git_dir = std::path::Path::new(cwd).join(".git");
    if !git_dir.exists() {
        return Vec::new();
    }
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok();
    let branch = head
        .as_deref()
        .and_then(|s| s.trim().strip_prefix("ref: refs/heads/"))
        .map(|b| b.to_string());
    let mut entry = serde_json::json!({
        "repoPath": cwd,
        "branches": [],
    });
    if let Some(b) = branch {
        entry["branches"] = serde_json::json!([{
            "branchName": b,
        }]);
    }
    vec![entry]
}

// ── Composition (schema reverse-engineered from
//    c1ea7999-005a-434f-bcf4-da8ddd9ff066) ─────────────────────

/// Desktop can open a composer only when `conversationState` and display
/// timestamps are present (v3 storage). Stub injects that only filled
/// `conversationState` still spin on "Loading chat".
pub(crate) fn composer_is_desktop_loadable(v: &serde_json::Value) -> bool {
    let cs_ok = v
        .get("conversationState")
        .and_then(|x| x.as_str())
        .map(|s| s.len() > 2)
        .unwrap_or(false);
    let last_updated = v.get("lastUpdatedAt").and_then(|x| x.as_i64()).unwrap_or(0);
    let headers_have_ts = v
        .get("fullConversationHeadersOnly")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.is_empty()
                || arr.iter().any(|h| {
                    h.get("createdAt")
                        .and_then(|x| x.as_str())
                        .map(|s| !s.is_empty())
                        .unwrap_or(false)
                })
        })
        .unwrap_or(true);
    cs_ok
        && last_updated > 0
        && headers_have_ts
        && composer_context_is_valid(v)
        && composer_agent_fields_valid(v)
}

fn composer_agent_fields_valid(v: &serde_json::Value) -> bool {
    for key in [
        "conversationMap",
        "codeBlockData",
        "usageData",
        "originalFileStates",
    ] {
        if !v.get(key).map(|x| x.is_object()).unwrap_or(false) {
            return false;
        }
    }
    v.get("capabilities").map(|x| x.is_array()).unwrap_or(false)
        && v.get("agentBackend").and_then(|x| x.as_str()).is_some()
}

/// Desktop `loadFromStorage` dereferences `context.fileSelections` unconditionally.
fn composer_context_is_valid(v: &serde_json::Value) -> bool {
    v.get("context")
        .and_then(|c| c.get("fileSelections"))
        .map(|x| x.is_array())
        .unwrap_or(false)
}

/// Empty composer `context` matching Desktop v3 schema (c1ea7999 snapshot).
pub fn default_composer_context() -> serde_json::Value {
    let mentions = serde_json::json!({
        "composers": {},
        "selectedCommits": {},
        "selectedPullRequests": {},
        "gitDiff": [],
        "gitDiffFromBranchToMain": [],
        "selectedImages": {},
        "selectedDocuments": {},
        "selectedVideos": {},
        "folderSelections": {},
        "fileSelections": {},
        "terminalFiles": {},
        "selections": {},
        "terminalSelections": {},
        "selectedDocs": {},
        "externalLinks": {},
        "diffHistory": [],
        "cursorRules": {},
        "cursorCommands": {},
        "uiElementSelections": [],
        "consoleLogs": [],
        "ideEditorsState": [],
        "gitPRDiffSelections": {},
        "subagentSelections": {},
        "browserSelections": {}
    });
    serde_json::json!({
        "composers": [],
        "selectedCommits": [],
        "selectedPullRequests": [],
        "selectedImages": [],
        "selectedDocuments": [],
        "selectedVideos": [],
        "folderSelections": [],
        "fileSelections": [],
        "selections": [],
        "terminalSelections": [],
        "selectedDocs": [],
        "externalLinks": [],
        "cursorRules": [],
        "cursorCommands": [],
        "gitPRDiffSelections": [],
        "subagentSelections": [],
        "browserSelections": [],
        "extraContext": [],
        "mentions": mentions,
    })
}

/// Agent-mode composer fields Desktop dereferences via `Object.entries` during load.
pub fn default_agent_composer_fields() -> serde_json::Value {
    serde_json::json!({
        "conversationMap": {},
        "codeBlockData": {},
        "usageData": {},
        "originalFileStates": {},
        "capabilities": [
            {"type": 15, "data": {"bubbleDataMap": "{}"}},
            {"type": 19, "data": {}},
            {"type": 33, "data": {}},
            {"type": 32, "data": {}},
            {"type": 23, "data": {}},
            {"type": 16, "data": {}},
            {"type": 24, "data": {}}
        ],
        "capabilityContexts": [],
        "todos": [],
        "queueItems": [],
        "isAgentic": true,
        "agentBackend": "cursor-agent",
        "isNAL": true,
        "isQueueExpanded": true,
        "promptContextUsageTree": {"schemaVersion": 1, "nodes": []},
        "promptTokenBreakdown": {
            "totalUsedTokens": 0,
            "maxTokens": 200000,
            "categories": []
        },
        "subComposerIds": [],
        "subagentComposerIds": [],
        "allAttachedFileCodeChunksUris": [],
        "newlyCreatedFiles": [],
        "newlyCreatedFolders": [],
    })
}

/// Merge [`default_agent_composer_fields`] + [`default_composer_context`] into `composer`.
pub fn ensure_desktop_loadable_composer(composer: &mut serde_json::Value) {
    let Some(obj) = composer.as_object_mut() else {
        return;
    };
    let defaults_ctx = default_composer_context();
    match obj.get_mut("context") {
        Some(existing) if existing.is_object() => {
            if let (Some(ec), Some(def)) = (existing.as_object_mut(), defaults_ctx.as_object()) {
                for (k, v) in def {
                    ec.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        _ => {
            obj.insert("context".into(), defaults_ctx);
        }
    }
    if let Some(agent) = default_agent_composer_fields().as_object() {
        for (k, v) in agent {
            obj.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

pub fn compose_composer_data(
    uuid: &str,
    name: &str,
    created_at_ms: i64,
    last_updated_ms: i64,
    workspace_identifier: &Option<serde_json::Value>,
    _project_slug: &str,
    tracked_git_repos: &[serde_json::Value],
    bubbles: &[LayerBubble],
    session_created_ms: i64,
) -> serde_json::Value {
    let session_base = if session_created_ms > 0 {
        session_created_ms
    } else {
        created_at_ms
    };
    let headers_only: Vec<serde_json::Value> = bubbles
        .iter()
        .enumerate()
        .map(|(idx, b)| {
            let bubble_id = bubble_id_for_layer3(uuid, b, idx);
            let display_ms = bubble_display_timestamp(session_base, idx, b.created_at_ms);
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
            let created_at_iso = ms_to_iso(display_ms);
            serde_json::json!({
                "bubbleId": bubble_id,
                "type": bubble_type,
                "createdAt": created_at_iso,
                "grouping": grouping,
            })
        })
        .collect();

    let mut root = serde_json::json!({
        "_v": 16,
        "composerId": uuid,
        "richText": "",
        "hasLoaded": true,
        "text": "",
        "name": name,
        "context": default_composer_context(),
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
        "trackedGitRepos": tracked_git_repos,
        "workspaceIdentifier": workspace_identifier.clone().unwrap_or(serde_json::Value::Null),
        "fullConversationHeadersOnly": headers_only,
        "status": "completed",
        "generatingBubbleIds": [],
        "isContinuationInProgress": false,
        "modelConfig": {
            "modelName": "default",
            "maxMode": false,
            "selectedModels": [{"modelId": "default", "parameters": []}],
        },
    });
    if created_at_ms > 0 {
        if let Some(obj) = root.as_object_mut() {
            obj.insert("createdAt".into(), serde_json::json!(created_at_ms));
            obj.insert(
                "conversationCheckpointLastUpdatedAt".into(),
                serde_json::json!(last_updated_ms.max(created_at_ms)),
            );
        }
    }
    ensure_desktop_loadable_composer(&mut root);
    root
}

pub fn compose_composer_header_entry(
    uuid: &str,
    name: &str,
    subtitle: &str,
    created_at_ms: i64,
    last_updated_ms: i64,
    workspace_identifier: &Option<serde_json::Value>,
    tracked_git_repos: &[serde_json::Value],
) -> serde_json::Value {
    // Real Cursor Desktop entries set `conversationCheckpointLast
    // UpdatedAt` to a value strictly ≥ `lastUpdatedAt`. We mirror
    // that with last_updated_ms so the Sidebar's "checkpoint" /
    // resume logic sees a consistent timeline even though we have
    // no real checkpoint data. Without this field, the Sidebar
    // reader (per the diff vs c1ea7999) refuses to render the row.
    serde_json::json!({
        "type": "head",
        "composerId": uuid,
        "name": name,
        "lastUpdatedAt": last_updated_ms,
        "conversationCheckpointLastUpdatedAt": last_updated_ms,
        "createdAt": created_at_ms,
        "unifiedMode": "agent",
        "forceMode": "edit",
        "hasUnreadMessages": false,
        "contextUsagePercent": 0.0,
        "totalLinesAdded": 0,
        "totalLinesRemoved": 0,
        "filesChangedCount": 0,
        "subtitle": subtitle,
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
        "trackedGitRepos": tracked_git_repos,
        "workspaceIdentifier": workspace_identifier.clone().unwrap_or(serde_json::Value::Null),
    })
}

pub fn compose_bubble_blobs(
    uuid: &str,
    bubbles: &[LayerBubble],
    session_created_ms: i64,
) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for (idx, b) in bubbles.iter().enumerate() {
        let bubble_id = bubble_id_for_layer3(uuid, b, idx);
        let display_ms = bubble_display_timestamp(session_created_ms, idx, b.created_at_ms);
        let created_at_iso = ms_to_iso(display_ms);
        let bubble_type = match b.role.as_str() {
            "user" => 1,
            _ => 2,
        };
        let body = match b.role.as_str() {
            "user" => {
                let display_text = super::canonical::clean_user_text(&b.text);
                let images_json = bubble_images_json(&b.images);
                serde_json::json!({
                    "_v": 3,
                    "type": bubble_type,
                    "bubbleId": bubble_id,
                    "text": display_text,
                    "richText": minimal_rich_text(&display_text),
                    "createdAt": created_at_iso,
                    "images": images_json,
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
                })
            }
            _ => {
                let mut obj = serde_json::json!({
                    "_v": 3,
                    "type": bubble_type,
                    "bubbleId": bubble_id,
                    "text": b.text,
                    "isAgentic": true,
                    "createdAt": created_at_iso,
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
                });
                if let Some(tool) = first_tool_former_data(&b.tool_calls) {
                    if let Some(map) = obj.as_object_mut() {
                        map.insert("toolFormerData".into(), tool);
                    }
                }
                obj
            }
        };
        out.push((bubble_id, body));
    }
    out
}

/// Desktop user bubbles carry `images[]` with data-URL `url` fields.
fn bubble_images_json(images: &[super::canonical::BubbleImage]) -> Vec<serde_json::Value> {
    images
        .iter()
        .map(|img| {
            serde_json::json!({
                "url": format!("data:{};base64,{}", img.mime_type, img.data_base64),
                "dimension": { "width": 0, "height": 0 },
            })
        })
        .collect()
}

/// Minimal `toolFormerData` for the first L2 tool-call (Desktop render hint).
fn first_tool_former_data(
    tool_calls: &[super::canonical::BubbleToolUse],
) -> Option<serde_json::Value> {
    let tc = tool_calls.first()?;
    let params = tc
        .input
        .as_ref()
        .and_then(|v| serde_json::to_string(v).ok())
        .unwrap_or_else(|| "{}".to_string());
    Some(serde_json::json!({
        "name": tc.name,
        "params": params,
        "status": "completed",
    }))
}

/// Stable bubble id for inject — keep `created_at_ms=0` in the hash so
/// re-inject does not rotate ids when we only add display timestamps.
fn bubble_id_for_layer3(uuid: &str, b: &LayerBubble, idx: usize) -> String {
    if !b.id.is_empty() {
        return b.id.clone();
    }
    deterministic_bubble_id(uuid, &b.role, 0, idx)
}

fn bubble_display_timestamp(session_base_ms: i64, idx: usize, bubble_ms: i64) -> i64 {
    if bubble_ms > 0 {
        return bubble_ms;
    }
    if session_base_ms > 0 {
        return session_base_ms + idx as i64;
    }
    0
}

/// Minimal Lexical `richText` JSON string for user bubbles (Desktop v3).
fn minimal_rich_text(text: &str) -> String {
    let escaped = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"{{"root":{{"children":[{{"children":[{{"detail":0,"format":0,"mode":"normal","style":"","text":{escaped},"type":"text","version":1}}],"direction":"ltr","format":"","indent":0,"type":"paragraph","version":1}}],"direction":"ltr","format":"","indent":0,"type":"root","version":1}}}}"#
    )
}

pub fn merge_composer_headers(
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

/// Desktop-equivalent sidebar soft delete: mark `isArchived`, clear title.
/// Returns `true` when an `allComposers` entry with matching `composerId`
/// was found and patched in place.
pub fn archive_composer_sidebar_entry(root: &mut serde_json::Value, uuid: &str) -> bool {
    let Some(all) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("allComposers"))
        .and_then(|v| v.as_array_mut())
    else {
        return false;
    };
    let mut found = false;
    for entry in all.iter_mut() {
        if entry.get("composerId").and_then(|c| c.as_str()) == Some(uuid) {
            if let Some(obj) = entry.as_object_mut() {
                obj.insert("isArchived".into(), serde_json::json!(true));
                obj.insert("name".into(), serde_json::json!(""));
                obj.insert("subtitle".into(), serde_json::json!(""));
            }
            found = true;
        }
    }
    found
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

/// Hex-encode the JSON-serialized form of `v` — what the `value_hex`
/// column of `Mutation` carries verbatim. `apply_mutation_inline`
/// decodes back to bytes before the SQLite INSERT.
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
///
/// `pub(crate)` since v0.2.2: previously private to `core::inject`, now
/// also called by [`super::canonical::read_layer1_bubbles_from_path`] so
/// L1 JSONL bubbles can participate in the 3-layer `merge_bubbles_three_way`
/// reconciliation. The algorithm is unchanged — callers that pass the
/// same `(uuid, role, ts_ms, ordinal)` tuple always get the same id back.
pub(crate) fn deterministic_bubble_id(
    uuid: &str,
    role: &str,
    ts_ms: i64,
    ordinal: usize,
) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_composer_sidebar_entry_sets_archived_and_clears_title() {
        let uuid = "11111111-1111-1111-1111-111111111111";
        let mut root = serde_json::json!({
            "allComposers": [
                {
                    "composerId": uuid,
                    "name": "Hello test",
                    "subtitle": "user msg",
                    "isArchived": false
                },
                {
                    "composerId": "other",
                    "name": "keep",
                    "isArchived": false
                }
            ]
        });
        assert!(archive_composer_sidebar_entry(&mut root, uuid));
        let entry = &root["allComposers"][0];
        assert_eq!(entry["isArchived"], true);
        assert_eq!(entry["name"], "");
        assert_eq!(entry["subtitle"], "");
        assert_eq!(root["allComposers"][1]["name"], "keep");
        assert!(!archive_composer_sidebar_entry(&mut root, "missing"));
    }

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
    fn default_composer_context_has_file_selections() {
        let ctx = default_composer_context();
        assert!(ctx
            .get("fileSelections")
            .and_then(|x| x.as_array())
            .is_some());
        assert!(ctx.get("mentions").is_some());
    }

    #[test]
    fn default_agent_composer_fields_has_object_maps() {
        let agent = default_agent_composer_fields();
        assert!(agent
            .get("conversationMap")
            .map(|x| x.is_object())
            .unwrap_or(false));
        assert!(agent
            .get("codeBlockData")
            .map(|x| x.is_object())
            .unwrap_or(false));
        assert!(agent
            .get("capabilities")
            .map(|x| x.is_array())
            .unwrap_or(false));
    }

    #[test]
    fn ensure_desktop_loadable_composer_fills_missing_agent_fields() {
        let mut composer = serde_json::json!({
            "composerId": "x",
            "context": {"fileSelections": []},
            "conversationState": "~abc",
            "lastUpdatedAt": 1,
            "fullConversationHeadersOnly": [{"bubbleId":"b","createdAt":"t","type":1}]
        });
        ensure_desktop_loadable_composer(&mut composer);
        assert!(composer_is_desktop_loadable(&composer));
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

    /// `build_workspace_identifier` must return the same id that
    /// Cursor's workspaceStorage uses for a cwd the user has
    /// opened. This test relies on the dev machine having opened
    /// `/home/eric/workspace/bettercursor` in Cursor (which our
    /// session list proves). If the assertion fails on a fresh
    /// checkout, run the test with `--ignored` skipped — the
    /// fallback path (MD5(cwd)) keeps the schema valid.
    #[test]
    #[ignore]
    fn build_workspace_identifier_returns_real_storage_hash() {
        let id =
            build_workspace_identifier("/home/eric/workspace/bettercursor").expect("non-empty cwd");
        let id_str = id.get("id").and_then(|x| x.as_str()).unwrap_or("");
        // Real hash from `ls ~/.config/Cursor/User/workspaceStorage/`.
        assert_eq!(
            id_str, "b9c96f3499915796f28905f2e97f8164",
            "workspaceIdentifier.id must match Cursor's workspaceStorage dir name"
        );
    }

    /// When cwd has no entry in workspaceStorage (e.g. a CLI session
    /// started in a folder the user has never opened in Cursor),
    /// the fallback must produce a syntactically valid 32-char hex
    /// (MD5 of cwd) — not an empty string or random gibberish.
    #[test]
    fn build_workspace_identifier_falls_back_to_md5() {
        // /tmp never appears in workspaceStorage on any sane install.
        let id =
            build_workspace_identifier("/tmp/nonexistent-workspace-12345").expect("non-empty cwd");
        let id_str = id.get("id").and_then(|x| x.as_str()).unwrap_or("");
        assert_eq!(id_str.len(), 32, "expected 32-char hex, got {id_str:?}");
        assert!(
            id_str.chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex digits only, got {id_str:?}"
        );
    }

    /// Empty cwd → no identifier (composer entry still goes through
    /// but without workspaceIdentifier — Cursor will treat the entry
    /// as workspace-orphan and skip Sidebar rendering).
    #[test]
    fn build_workspace_identifier_empty_returns_none() {
        assert!(build_workspace_identifier("").is_none());
        assert!(build_workspace_identifier("   ").is_none());
    }
}
