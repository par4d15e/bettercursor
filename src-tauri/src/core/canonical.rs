//! bettercursor canonical — merge sessions across the 3 storage layers.
//!
//! Scans Layer 1 (JSONL), Layer 2 (store.db), Layer 3 (state.vscdb) and
//! produces a single `Vec<CanonicalSession>` keyed by `uuid`.
//!
//! See PRD §4.1 for the canonical record schema.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::paths;
use super::storage;

// ── Conversation model (Layer 1 JSONL → UI) ───────────────────

/// One executed tool call inside an assistant bubble.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BubbleToolUse {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
}

/// One JSONL line, surfaced to the UI as one bubble.
///
/// `role` is `"user"` or `"assistant"` (empty string for orphan events
/// such as `turn_ended` / errors, which are filtered out downstream).
///
/// `id` and `created_at_ms` (both `#[serde(default)]`) were added in
/// v0.2.2 to support 3-layer bubble reconciliation in `merge_bubbles_three_way`.
/// L1 (JSONL) bubbles have id filled by `inject::deterministic_bubble_id`
/// (SHA256-derived 36-char GUID keyed on uuid|role|ts|ordinal) and
/// `created_at_ms` populated from the line's `timestamp` field (0 when
/// missing). The two new fields are intentionally defaulted so existing
/// `Conversation { bubbles: [...] }` payloads — including the unit tests
/// in this file that construct `Bubble { role, text, tool_calls, files }`
/// inline — keep deserializing cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bubble {
    /// Stable 36-char GUID (SHA256-derived). Defaulted so missing
    /// fields in old serialized payloads don't break decoding.
    #[serde(default)]
    pub id: String,
    pub role: String,
    pub text: String,
    pub tool_calls: Vec<BubbleToolUse>,
    pub files: Vec<String>,
    /// Epoch milliseconds. Defaulted because L1 JSONL often has no
    /// reliable timestamp; L2/L3 bubbles have it set.
    #[serde(default)]
    pub created_at_ms: i64,
    /// v0.3.0: optional reference to the previous bubble in the
    /// conversation chain. Populated by the L2 / L3 readers when they
    /// can infer a parent (most L1 JSONL streams don't carry an
    /// explicit parent ref). v0.3.0 first cut leaves this `None` for
    /// L1-derived bubbles — see the v0.3.0 plan note about
    /// "parent_bubble_id v0.3.0 全部 None, v0.3.1 启发式回填".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_bubble_id: Option<String>,
}

/// Full transcript for a single session uuid, resolved from Layer 1.
///
/// `source_path` is `None` when no Layer 1 JSONL was found — the session
/// may live only on Layers 2 / 3 in that case (not yet loadable in v0.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub uuid: String,
    pub bubbles: Vec<Bubble>,
    pub source_path: Option<String>,
    pub total_lines: usize,
    pub parse_errors: usize,
}

// ── Types (mirror PRD §4.1) ───────────────────────────────────

/// One storage layer that produced a session record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceLayer {
    Mac,
    LinuxCli,
    LinuxDesktop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub last_seen_at: i64,
    pub layer: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<SourceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux_cli: Option<SourceInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux_desktop: Option<SourceInfo>,
}

impl Sources {
    /// v0.3.0: pick a stable endpoint-kind label for this session —
    /// `mac` wins over `linux_desktop` wins over `linux_cli` so the
    /// remote pull side can tell which "flavor" of Cursor produced
    /// the row without having to dedup multiple sources.
    pub fn preferred_endpoint_kind(&self) -> String {
        if self.mac.is_some() {
            "mac".to_string()
        } else if self.linux_desktop.is_some() {
            "linux_desktop".to_string()
        } else if self.linux_cli.is_some() {
            "linux_cli".to_string()
        } else {
            "unknown".to_string()
        }
    }

    /// v0.3.0: pick a stable source path (mac > linux_desktop >
    /// linux_cli) — used by `UnifiedDb::rebuild_from_cursor_state` so
    /// the `sessions.source_path` column always reflects one concrete
    /// on-disk location even when multiple layers co-exist.
    pub fn preferred_source_path(&self) -> String {
        self.mac
            .as_ref()
            .or(self.linux_desktop.as_ref())
            .or(self.linux_cli.as_ref())
            .map(|info| info.path.clone())
            .unwrap_or_default()
    }
}

/// v0.3.0: Layer 3 `composerData` JSON captured verbatim + a
/// normalized subset. The full JSON is what Cursor itself writes into
/// `state.vscdb::cursorDiskKV[composerData:<uuid>]`; the subset is a
/// v0.3.0 first-cut convenience copy (we don't synthesize a real
/// subset yet — PR-2 v4 codec round-trips full_json through the
/// wire format unchanged and reads back via unified.db).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ComposerData {
    pub full_json: String,
    pub subset_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalSession {
    pub uuid: String,
    pub project_slug: String,
    pub project_path: String,
    pub chat_root: String,
    pub name: String,
    pub last_updated_at: i64,
    pub bubble_count: u32,
    pub is_empty_draft: bool,
    /// True when we detected a data-correctness issue that makes this
    /// session unusable (`cursor-agent --resume` would fail, conversation
    /// can't be loaded, etc.). Always paired with a non-empty
    /// `broken_reason`.
    #[serde(default)]
    pub is_broken: bool,
    /// Human-readable explanation of `is_broken`, e.g.
    /// "Layer 2 latestRootBlobId is empty — `--resume` will fail".
    /// `None` when `is_broken == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken_reason: Option<String>,
    pub sources: Sources,
    pub first_user_message_preview: String,
    pub files_referenced: Vec<String>,
    /// Concatenated text from the conversation (first ~2 KB), used for
    /// full-content search on the frontend. Populated from Layer 1.
    #[serde(default)]
    pub indexable_text: String,
    /// True iff Layer 3 (state.vscdb / cursorDiskKV['composerData:<uuid>'])
    /// contains a corresponding entry for this session.
    /// Drives the "注入 Desktop" UI button: when false, this CLI-originated
    /// session is invisible to Electron Cursor's Sidebar.
    #[serde(default)]
    pub layer_3_present: bool,
    /// v0.3.0: Layer 3 composerData full JSON + subset, captured at
    /// scan time so unified.db write paths don't need to re-open
    /// state.vscdb later. `None` when the session has no Layer 3 row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer_data: Option<ComposerData>,
    /// v0.3.0: the `composerId` field Cursor writes into the L3
    /// composerData JSON. Equals the session `uuid` today but kept
    /// separate so we can pivot if Cursor ever splits the two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer_id: Option<String>,
}

// ── Entry point ───────────────────────────────────────────────

/// Scan all 3 storage layers on the current host and return merged sessions.
pub fn scan_all() -> Result<Vec<CanonicalSession>> {
    let mut by_uuid: HashMap<String, CanonicalSession> = HashMap::new();

    // Layer 1 (JSONL) is **scanned last** and is treated as a transcript
    // only — it does not stamp any source-layer tag, because the same
    // file layout is produced by both Cursor Desktop Electron and the
    // CLI. See #87 for the bug it caused when stamped as `linux_cli`.
    //
    // Layer 3 (state.vscdb) and Layer 2 (store.db) carry **origin**
    // information:
    //   - state.vscdb → Desktop (or Mac)
    //   - store.db   → CLI only
    // We run them in priority order so the higher-fidelity writer
    // wins: Layer 3 first (sets linux_desktop), Layer 2 second (sets
    // linux_cli), Layer 1 last (fills preview/indexable_text without
    // touching sources).
    scan_layer3_into(&mut by_uuid);
    scan_layer2_into(&mut by_uuid);
    scan_layer1_into(&mut by_uuid);

    // Reconcile derived fields. `merge_source` historically set
    // `is_empty_draft` whenever it was called with a real source
    // layer; with the new ordering (#87/#88) Layer 1 seeds entries
    // directly and bypasses that check, so we walk all entries once
    // at the end and recompute the derived fields uniformly.
    for entry in by_uuid.values_mut() {
        // `is_empty_draft` = zero bubbles AND no source at any layer.
        // A session that has Layer 1/2/3 data but `bubble_count == 0`
        // is **not** empty — the conversation exists, we just can't
        // count it (e.g. older JSONL format). Only mark empty when
        // we have nothing.
        let has_any_source = entry.sources.mac.is_some()
            || entry.sources.linux_cli.is_some()
            || entry.sources.linux_desktop.is_some();
        entry.is_empty_draft = entry.bubble_count == 0 && !has_any_source;
    }

    // Sort by last_updated_at descending.
    let mut out: Vec<CanonicalSession> = by_uuid.into_values().collect();
    out.sort_by(|a, b| b.last_updated_at.cmp(&a.last_updated_at));
    Ok(out)
}

// ── Layer 1: JSONL transcripts ────────────────────────────────

fn scan_layer1_into(by_uuid: &mut HashMap<String, CanonicalSession>) {
    let projects_dir = paths::cursor_projects_dir();
    if !projects_dir.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&projects_dir) else {
        return;
    };

    for project_entry in entries.flatten() {
        let project_slug = project_entry.file_name().to_string_lossy().to_string();
        let transcripts_dir = project_entry.path().join("agent-transcripts");
        if !transcripts_dir.is_dir() {
            continue;
        }

        let Ok(transcript_entries) = std::fs::read_dir(&transcripts_dir) else {
            continue;
        };

        for t_entry in transcript_entries.flatten() {
            let uuid = t_entry
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".jsonl")
                .to_string();

            // Cursor stores each session as either:
            //   (a) a directory `<uuid>/<uuid>.jsonl`, or
            //   (b) a flat file `<uuid>.jsonl` (older layout)
            // Handle both so we don't read a directory as a file and get
            // an empty preview for every session.
            let entry_path = t_entry.path();
            let (jsonl_path, display_path) = if entry_path.is_dir() {
                let p = entry_path.join(format!("{uuid}.jsonl"));
                let display = p.display().to_string();
                (p, display)
            } else {
                let display = entry_path.display().to_string();
                (entry_path, display)
            };
            let modified = file_mtime_ms(&jsonl_path).unwrap_or(0);
            let (preview, _files) = read_jsonl_preview(&jsonl_path);
            let indexable = read_jsonl_indexable(&jsonl_path);

            // Layer 1 (JSONL transcript) is **not** a proof of origin —
            // Cursor Desktop Electron writes the same JSONL files when
            // it hosts a session, so we can't distinguish a CLI
            // transcript from a Desktop one by file presence alone. The
            // source-layer tag therefore comes from Layer 2 (store.db,
            // CLI-only) or Layer 3 (state.vscdb, Desktop-only). Layer 1
            // only seeds metadata that's uniquely available here:
            //   - first_user_message_preview (only Layer 1 has user
            //     message text, not the blob IDs Layer 2/3 store)
            //   - project_slug (Layer 1 directory is named after the
            //     project; we fall back to this when Layer 2/3 don't
            //     carry a workspaceIdentifier)
            //   - indexable_text (per-conversation full-text index)
            //   - last_updated_at (file mtime as a coarse floor)
            // Do NOT call `merge_source` here with any SourceLayer —
            // doing so previously caused cfa4177f (a Desktop session
            // with no Layer 2) to be incorrectly tagged
            // `linux_cli:L1 + linux_desktop:L3` (#87).
            let entry = by_uuid.entry(uuid.clone()).or_insert_with(|| CanonicalSession {
                uuid: uuid.clone(),
                project_slug: project_slug.clone(),
                project_path: String::new(),
                chat_root: String::new(),
                name: String::new(),
                last_updated_at: modified,
                bubble_count: 0,
                is_empty_draft: true,
                is_broken: false,
                broken_reason: None,
                sources: Sources::default(),
                first_user_message_preview: String::new(),
                files_referenced: vec![],
                indexable_text: String::new(),
                layer_3_present: false,
                composer_data: None,
                composer_id: None,
            });
            if !preview.is_empty() && entry.first_user_message_preview.is_empty() {
                entry.first_user_message_preview = preview;
            }
            if entry.project_slug.is_empty() {
                entry.project_slug = project_slug.clone();
            }
            if modified > entry.last_updated_at {
                entry.last_updated_at = modified;
            }
            if entry.indexable_text.is_empty() {
                entry.indexable_text = indexable;
            }
        }
    }
}

fn read_jsonl_preview(path: &Path) -> (String, Vec<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return (String::new(), vec![]);
    };
    let mut preview = String::new();
    let mut files = Vec::new();

    // Take only the first user-role message for preview (fast).
    //
    // Real cursor-agent JSONL schema (verified 2026/07):
    //   {
    //     "role": "user" | "assistant",
    //     "message": {
    //       "content": [
    //         { "type": "text", "text": "<user_query>\n...\n</user_query>" },
    //         { "type": "tool_use", "name": "Glob", "input": {...} }
    //       ]
    //     },
    //     "attachments": [{"name": "foo.py"}, ...]   // optional, top-level
    //   }
    //
    // Older / other layers may put `content` directly on the root, so we
    // also try that as a fallback before giving up.
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        let role = v.get("role").and_then(|r| r.as_str());
        if role == Some("user") && preview.is_empty() {
            // 1) New schema: content is nested under message.content[]
            let nested_text = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .and_then(|arr| {
                    arr.iter().find_map(|item| {
                        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                            item.get("text").and_then(|t| t.as_str())
                        } else {
                            None
                        }
                    })
                });

            // 2) Older / fallback: content directly on root, may be a
            //    string or an array of text parts.
            let root_text = || -> Option<String> {
                if let Some(s) = v.get("content").and_then(|c| c.as_str()) {
                    return Some(s.to_string());
                }
                v.get("content")
                    .and_then(|c| c.as_array())
                    .and_then(|arr| {
                        arr.iter().find_map(|item| {
                            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                item.get("text").and_then(|t| t.as_str())
                            } else {
                                None
                            }
                        })
                        .map(|s| s.to_string())
                    })
            };

            let raw = nested_text.map(str::to_string).or_else(root_text);
            if let Some(text) = raw {
                preview = clean_user_preview(&text);
            }
        }

        // Top-level attachments (any message, any role).
        if let Some(attachments) = v.get("attachments").and_then(|a| a.as_array()) {
            for a in attachments {
                if let Some(name) = a.get("name").and_then(|n| n.as_str()) {
                    files.push(name.to_string());
                }
            }
        }
    }
    (preview, files)
}

/// Strip wrapper tags (`<user_query>...</user_query>`, `<timestamp>...</timestamp>`)
/// from a cursor-agent user message and take the first non-empty line, capped at
/// 120 characters. Returns an empty string if nothing usable remains.
fn clean_user_preview(text: &str) -> String {
    let mut s = text.trim().to_string();

    // Drop leading <timestamp>...</timestamp> block if present.
    if s.starts_with("<timestamp>") {
        if let Some(idx) = s.find("</timestamp>") {
            s = s[idx + "</timestamp>".len()..].trim().to_string();
        }
    }

    // Strip <user_query>...</user_query> wrappers.
    let open = "<user_query>";
    let close = "</user_query>";
    if s.starts_with(open) {
        s = s[open.len()..].to_string();
    }
    if s.ends_with(close) {
        s = s[..s.len() - close.len()].to_string();
    }

    // First non-empty line, capped at 120 chars.
    let first = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        out.push('…');
    }
    out
}

// ── Layer 2: cursor-agent CLI store.db ────────────────────────

fn scan_layer2_into(by_uuid: &mut HashMap<String, CanonicalSession>) {
    let chats_dir = paths::chats_dir();
    if !chats_dir.exists() {
        return;
    }
    let Ok(chat_roots) = std::fs::read_dir(&chats_dir) else {
        return;
    };

    for root_entry in chat_roots.flatten() {
        let chat_root = root_entry.file_name().to_string_lossy().to_string();
        let Ok(chat_dirs) = std::fs::read_dir(root_entry.path()) else {
            continue;
        };

        for chat_entry in chat_dirs.flatten() {
            let uuid = chat_entry.file_name().to_string_lossy().to_string();
            let store_db = chat_entry.path().join("store.db");
            if !store_db.is_file() {
                continue;
            }

            let (name, created_at, blob_count, latest_root_blob_id) =
                read_store_db_meta(&store_db);
            let modified = file_mtime_ms(&store_db).unwrap_or(created_at);
            let project_slug = derive_slug_from_chat_root(&chat_root);

            merge_source(
                by_uuid,
                &uuid,
                SourceInfo {
                    last_seen_at: modified,
                    layer: "2".into(),
                    path: store_db.display().to_string(),
                },
                SourceLayer::LinuxCli,
                &project_slug,
                name,
                modified,
                String::new(),
            );

            // Update bubble_count, last_updated_at, and broken-state on
            // the merged entry.
            if let Some(entry) = by_uuid.get_mut(&uuid) {
                entry.bubble_count = entry.bubble_count.max(blob_count);
                if modified > entry.last_updated_at {
                    entry.last_updated_at = modified;
                }
                if entry.chat_root.is_empty() {
                    entry.chat_root = chat_root.clone();
                }
                // "Broken" rule: Layer 2 store.db exists, has blobs, but
                // the session's `latestRootBlobId` is the empty string
                // (a known cursor-agent data-loss mode that the legacy
                // Python daemon's `adapter/fix_orphan_sessions.py` used
                // to repair). v0.1 only surfaces this; v0.2 will add a
                // "修复" button that re-points the root.
                if matches!(latest_root_blob_id.as_deref(), Some(s) if s.is_empty())
                    && !entry.is_broken
                {
                    entry.is_broken = true;
                    entry.broken_reason = Some(
                        "Layer 2 latestRootBlobId 是空字符串 — `cursor-agent --resume` 会失败".to_string(),
                    );
                }
            }
        }
    }
}

fn read_store_db_meta(path: &Path) -> (String, i64, u32, Option<String>) {
    let Ok(r) = storage::open_read(path) else {
        return (String::new(), 0, 0, None);
    };

    // Read meta[0] (agent session metadata).
    let (name, created_at, latest_root_blob_id) = match r.get_json("0", "meta") {
        Ok(Some(v)) => {
            let name = v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = v
                .get("createdAt")
                .and_then(|x| x.as_i64())
                .unwrap_or(0);
            let root = v
                .get("latestRootBlobId")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            (name, created_at, root)
        }
        _ => (String::new(), 0, None),
    };

    // Count blobs.
    let blob_count = r.list_blob_ids().map(|v| v.len() as u32).unwrap_or(0);

    (name, created_at, blob_count, latest_root_blob_id)
}

fn derive_slug_from_chat_root(chat_root: &str) -> String {
    // Layer 2's `~/.cursor/chats/<md5(cwd)>/<uuid>/store.db` uses
    // md5(cwd) as the project bucket name. md5 is not directly
    // reversible, so we go through Cursor's own bookkeeping:
    //   - `~/.config/Cursor/User/workspaceStorage/<hash>/workspace.json`
    //     records each opened folder's URI. md5(folder_path) == chat_root
    //     is the canonical mapping.
    //   - When a match exists, return the human-readable path itself
    //     (sanitized) instead of the workspaceStorage hash — matches
    //     how `~/.cursor/projects/` slugs look to the user.
    //   - When no match exists, fall back to `chat-<md5>` so the session
    //     is still uniquely groupable; it just won't merge with the
    //     real project. (#99/#100)
    if let Some(slug) = reverse_chat_root_via_workspace_storage(chat_root) {
        return slug;
    }
    format!("chat-{chat_root}")
}

/// Reverse-lookup via `~/.config/Cursor/User/workspaceStorage/<hash>/workspace.json`.
/// If md5 of any `folder` URI matches the chat_root, return the
/// sanitized path as the project slug. This is the canonical mapping
/// because Cursor itself stores `folder` URIs there — md5(folder) is
/// exactly the chat_root the Layer 2 store.db lives under.
fn reverse_chat_root_via_workspace_storage(chat_root: &str) -> Option<String> {
    let ws_dir = paths::workspace_storage_dir().ok()?;
    if !ws_dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(&ws_dir).ok()?;
    for entry in entries.flatten() {
        let ws_json = entry.path().join("workspace.json");
        if !ws_json.is_file() {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&ws_json) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { continue };
        let folder = v.get("folder").and_then(|x| x.as_str()).unwrap_or("");
        let path = folder.strip_prefix("file://").unwrap_or(folder).trim_end_matches('/');
        if path.is_empty() {
            continue;
        }
        let computed = paths::chat_root_for(path);
        if computed == chat_root {
            // Use the workspaceStorage hash as the slug — it matches
            // what Layer 3 `workspaceIdentifier.id` references, so all
            // three layers converge on the same group. The user sees
            // the hash in the sidebar but it's stable and clickable.
            return Some(entry.file_name().to_string_lossy().into_owned());
        }
    }
    None
}

// ── Layer 3: Electron state.vscdb ─────────────────────────────

/// Extract the project path from a `composerData` value's
/// `workspaceIdentifier`. Cursor writes it as either a bare string
/// (older / simpler form) or as a full URI object — both shapes
/// are seen in the wild on 2026/07 versions:
///
///   (a) `"workspaceIdentifier": "/home/eric/workspace/foo"`
///   (b) `"workspaceIdentifier": {
///          "id": "a2a619...",
///          "uri": {
///            "fsPath": "/home/eric/workspace/foo",
///            "external": "file:///home/eric/workspace/foo",
///            "path":    "/home/eric/workspace/foo",
///            "scheme":  "file"
///          }
///        }`
///
/// Returns empty string when neither shape is present (some
/// legacy / synthetic entries we inject have `null`).
fn extract_workspace_path(v: &serde_json::Value) -> String {
    let Some(wi) = v.get("workspaceIdentifier") else {
        return String::new();
    };
    if let Some(s) = wi.as_str() {
        return s.to_string();
    }
    if let Some(obj) = wi.as_object() {
        if let Some(uri) = obj.get("uri").and_then(|u| u.as_object()) {
            for key in ["fsPath", "path", "external"] {
                if let Some(s) = uri.get(key).and_then(|x| x.as_str()) {
                    // Strip `file://` scheme prefix if present.
                    return s.strip_prefix("file://").unwrap_or(s).to_string();
                }
            }
        }
    }
    String::new()
}

fn scan_layer3_into(by_uuid: &mut HashMap<String, CanonicalSession>) {
    let global_db = match paths::global_db_path() {
        Ok(p) if p.exists() => p,
        _ => return,
    };
    let Ok(r) = storage::open_read(&global_db) else {
        return;
    };

    // Layer 3 has two relevant tables:
    //   - ItemTable: key `composer.composerData` → JSON with all composer headers
    //   - cursorDiskKV: keys like `composerData:<uuid>`, `bubbleId:<uuid>:<bid>`

    // Try the legacy single-blob location first.
    if let Ok(Some(blob)) = r.get_item_binary("composer.composerData", "ItemTable") {
        if let Ok(text) = std::str::from_utf8(&blob) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                walk_composer_data(&v, by_uuid, &global_db, SourceLayer::Mac);
            }
        }
    }

    // Then the per-composer location.
    if let Ok(keys) = r.list_keys("composerData:", "cursorDiskKV") {
        for key in keys {
            let Some(uuid) = key.strip_prefix("composerData:") else { continue };
            if let Ok(Some(v)) = r.get_json(&key, "cursorDiskKV") {
                let modified = file_mtime_ms(&global_db).unwrap_or(0);
                let name = v
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let project_path = extract_workspace_path(&v);
                let project_slug = if !project_path.is_empty() {
                    paths::sanitize_project_path(&project_path)
                } else {
                    // Cursor Desktop can create a session in an empty
                    // window (no folder opened yet) — those rows have
                    // no `workspaceIdentifier.uri.fsPath`. The previous
                    // fallback `desktop-<uuid>` looked like a real
                    // Cursor project name and confused users (#99).
                    // We collapse all such orphans into a single
                    // `"no-workspace"` group slug so the left tree can
                    // render one collapsible section instead of 30+
                    // unique-looking rows. Per-session identity is
                    // preserved via `uuid` (which is what the rest of
                    // the codebase uses for selection / resume).
                    "no-workspace".to_string()
                };
                let created_at = v
                    .get("createdAt")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                // `lastUpdatedAt` is Cursor's own per-session ms timestamp —
                // the real "this session was last touched at". Use it for
                // `last_updated_at` instead of the global state.vscdb mtime
                // (which is shared by ALL sessions and changes on every
                // write, making the sidebar order jitter — #102).
                let json_last_updated_at = v
                    .get("lastUpdatedAt")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                let last_updated_at = json_last_updated_at.max(created_at);
                let bubble_count = v
                    .get("bubbleCount")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0) as u32;

                // macOS only if we're actually on macOS; otherwise treat as linux_desktop.
                let source = if cfg!(target_os = "macos") {
                    SourceLayer::Mac
                } else {
                    SourceLayer::LinuxDesktop
                };

                merge_source(
                    by_uuid,
                    uuid,
                    SourceInfo {
                        last_seen_at: modified,
                        layer: "3".into(),
                        path: global_db.display().to_string(),
                    },
                    source.clone(),
                    &project_slug,
                    name,
                    last_updated_at,
                    String::new(),
                );
                if let Some(entry) = by_uuid.get_mut(uuid) {
                    entry.bubble_count = entry.bubble_count.max(bubble_count);
                    if entry.project_path.is_empty() && !project_path.is_empty() {
                        entry.project_path = project_path;
                    }
                    // Layer 3 entry exists for this uuid — drives the
                    // "注入 Desktop" UI button (false means we should
                    // offer to synthesize one from Layer 2 + JSONL).
                    entry.layer_3_present = true;
                    // v0.3.0: capture the full composerData JSON so
                    // unified.db write paths don't need to re-open
                    // state.vscdb later. subset_json mirrors full_json
                    // for now — v0.3.0 first cut doesn't synthesize a
                    // real subset, the v4 codec round-trips the full
                    // body verbatim.
                    let full_json = serde_json::to_string(&v).unwrap_or_default();
                    entry.composer_data = Some(ComposerData {
                        full_json: full_json.clone(),
                        subset_json: full_json,
                    });
                    // composerId == uuid today; kept separate so we can
                    // pivot if Cursor ever splits the two.
                    entry.composer_id = Some(uuid.to_string());
                }
            }
        }
    }
}

fn walk_composer_data(
    v: &serde_json::Value,
    by_uuid: &mut HashMap<String, CanonicalSession>,
    global_db: &Path,
    source: SourceLayer,
) {
    let Some(all_composers) = v.get("allComposers").and_then(|x| x.as_array()) else {
        return;
    };
    let modified = file_mtime_ms(global_db).unwrap_or(0);
    for c in all_composers {
        let Some(uuid) = c.get("composerId").and_then(|x| x.as_str()) else { continue };
        let name = c.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let workspace = extract_workspace_path(c);
        let project_slug = if !workspace.is_empty() {
            paths::sanitize_project_path(&workspace)
        } else {
            format!("desktop-{uuid}")
        };
        let created_at = c.get("createdAt").and_then(|x| x.as_i64()).unwrap_or(0);
        // Prefer the per-session `lastUpdatedAt` over the global mtime
        // for the same reason as `scan_layer3_into` above (#102):
        // allComposers live in a shared table; their mtimes are useless
        // for ordering by recency.
        let json_last_updated_at = c
            .get("lastUpdatedAt")
            .and_then(|x| x.as_i64())
            .unwrap_or(0);
        let last_updated_at = json_last_updated_at.max(created_at);
        merge_source(
            by_uuid,
            uuid,
            SourceInfo {
                last_seen_at: modified,
                layer: "3".into(),
                path: global_db.display().to_string(),
            },
            source.clone(),
            &project_slug,
            name,
            last_updated_at,
            String::new(),
        );
    }
}

// ── Merge helper ──────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn merge_source(
    by_uuid: &mut HashMap<String, CanonicalSession>,
    uuid: &str,
    info: SourceInfo,
    layer: SourceLayer,
    project_slug: &str,
    name: String,
    last_updated_at: i64,
    first_user_message_preview: String,
) {
    let entry = by_uuid.entry(uuid.to_string()).or_insert_with(|| CanonicalSession {
        uuid: uuid.to_string(),
        project_slug: project_slug.to_string(),
        project_path: String::new(),
        chat_root: String::new(),
        name: name.clone(),
        last_updated_at,
        bubble_count: 0,
        is_empty_draft: false,
        is_broken: false,
        broken_reason: None,
        sources: Sources::default(),
        first_user_message_preview: first_user_message_preview.clone(),
        files_referenced: vec![],
        indexable_text: String::new(),
        layer_3_present: false,
        composer_data: None,
        composer_id: None,
    });

    let slot = match layer {
        SourceLayer::Mac => &mut entry.sources.mac,
        SourceLayer::LinuxCli => &mut entry.sources.linux_cli,
        SourceLayer::LinuxDesktop => &mut entry.sources.linux_desktop,
    };
    if slot.is_none() {
        *slot = Some(info);
    }

    if !name.is_empty() && (entry.name.is_empty() || entry.name == "New Agent") {
        entry.name = name;
    }
    if !first_user_message_preview.is_empty() && entry.first_user_message_preview.is_empty() {
        entry.first_user_message_preview = first_user_message_preview;
    }
    // `indexable_text` is computed once per JSONL and is read-only across
    // merges — first-writer wins (currently always Layer 1).
    // (intentionally not merged here; populated by `read_jsonl_indexable`
    //  invoked from `scan_layer1_into`.)
    if entry.project_slug.is_empty() {
        entry.project_slug = project_slug.to_string();
    }
    if last_updated_at > entry.last_updated_at {
        entry.last_updated_at = last_updated_at;
    }
    // NOTE: `is_empty_draft` is reconciled once at the end of
    // `scan_all` (#87/#88) so Layer 1's direct entry insertion is
    // also reflected. Don't recompute it here.
}

// ── Helpers ───────────────────────────────────────────────────

fn file_mtime_ms(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let since_epoch = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(since_epoch.as_millis() as i64)
}

// Silence unused import warnings for items used only on certain platforms.
#[allow(dead_code)]
fn _path_helper(p: PathBuf) -> PathBuf {
    p
}

// ── Layer 1 conversation reader ──────────────────────────────

/// Read all bubbles from the Layer 1 JSONL for `uuid`.
///
/// Returns an empty `Conversation` (with `source_path = None`) when no
/// JSONL exists. Lines that fail to parse are counted but never panic.
///
/// v0.2.2: this function now reads from all three layers (Layer 1
/// JSONL + Layer 2 store.db + Layer 3 state.vscdb) and merges them via
/// [`merge_bubbles_three_way`]. The signature is unchanged — the frontend
/// still calls `get_conversation(uuid)` and gets back a single
/// `Conversation` with the merged `bubbles` array.
pub fn read_conversation(uuid: &str) -> Conversation {
    let cwd = "";
    let jsonl_path = paths::find_layer1_jsonl_for(uuid);
    let (l1, total_lines, parse_errors) = match jsonl_path.as_ref() {
        None => (Vec::new(), 0usize, 0usize),
        Some(path) => read_layer1_bubbles_from_path(uuid, path),
    };
    let l2 = read_layer2_bubbles(uuid, cwd);
    let l3 = read_layer3_bubbles(uuid);
    let merged = merge_bubbles_three_way(l1, l2, l3);
    Conversation {
        uuid: uuid.to_string(),
        bubbles: merged,
        source_path: jsonl_path.map(|p| p.display().to_string()),
        total_lines,
        parse_errors,
    }
}

/// Read all bubbles from an already-resolved JSONL file path.
///
/// Useful for tests and for callers that already know the file location.
pub fn read_conversation_from_path(uuid: &str, path: &Path) -> Conversation {
    let (bubbles, total_lines, parse_errors) = match std::fs::read_to_string(path) {
        Ok(body) => read_layer1_bubbles_from_body(uuid, &body),
        Err(_) => (Vec::new(), 0usize, 0usize),
    };
    Conversation {
        uuid: uuid.to_string(),
        bubbles,
        source_path: Some(path.display().to_string()),
        total_lines,
        parse_errors,
    }
}

/// v0.2.2: parse raw JSONL bytes into a [`Vec<Bubble>`] plus counters.
/// Split out from `read_conversation_from_path` so the 3-layer merge in
/// [`read_conversation`] can reuse the exact same L1 logic. The
/// `Bubble.id` is filled by [`super::inject::deterministic_bubble_id`]
/// keyed on `(uuid, role, created_at_ms, ordinal)` so L1 bubbles can
/// participate in [`merge_bubbles_three_way`] by id.
pub(crate) fn read_layer1_bubbles_from_body(uuid: &str, body: &str) -> (Vec<Bubble>, usize, usize) {
    let mut bubbles: Vec<Bubble> = vec![];
    let mut total_lines = 0usize;
    let mut parse_errors = 0usize;

    for (ordinal, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        total_lines += 1;

        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            parse_errors += 1;
            continue;
        };

        let role = v
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        // Resolve the `[{type, text|tool_use}]` content array, in this order:
        //  1. nested schema:  v.message.content
        //  2. legacy schema:  v.content
        let content_arr = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .or_else(|| v.get("content").and_then(|c| c.as_array()));

        let mut text_parts: Vec<String> = vec![];
        let mut tool_calls: Vec<BubbleToolUse> = vec![];

        if let Some(arr) = content_arr {
            for item in arr {
                let typ = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match typ {
                    "text" => {
                        if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                            // User bubbles have wrapper tags — strip those.
                            // Assistant bubbles stay verbatim (markdown).
                            let cleaned = if role == "user" {
                                clean_user_text(t)
                            } else {
                                t.to_string()
                            };
                            if !cleaned.is_empty() {
                                text_parts.push(cleaned);
                            }
                        }
                    }
                    "tool_use" => {
                        let name = item
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let input = item.get("input").cloned();
                        if !name.is_empty() {
                            tool_calls.push(BubbleToolUse { name, input });
                        }
                    }
                    _ => {}
                }
            }
        } else {
            // Legacy: `content` may be a plain string.
            let raw = v
                .get("content")
                .and_then(|c| c.as_str())
                .or_else(|| {
                    v.get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_str())
                })
                .unwrap_or("");
            if !raw.is_empty() {
                text_parts.push(if role == "user" {
                    clean_user_text(raw)
                } else {
                    raw.to_string()
                });
            }
        }

        let files = v
            .get("attachments")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| {
                        a.get("name")
                            .and_then(|n| n.as_str())
                            .map(str::to_string)
                    })
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();

        // Skip structural events (turn_ended / error) that produced no
        // user-visible bubble.
        if text_parts.is_empty() && tool_calls.is_empty() && files.is_empty() {
            continue;
        }

        let created_at_ms = v
            .get("timestamp")
            .and_then(|t| t.as_i64())
            .unwrap_or(0);
        let id = super::inject::deterministic_bubble_id(uuid, &role, created_at_ms, ordinal);

        bubbles.push(Bubble {
            id,
            role,
            text: text_parts.join("\n\n"),
            tool_calls,
            files,
            created_at_ms,
            parent_bubble_id: None,
        });
    }

    (bubbles, total_lines, parse_errors)
}

/// v0.2.2: read all L1 JSONL bubbles for `uuid`, returning a
/// `(bubbles, total_lines, parse_errors)` triple. Returns `(Vec::new(),
/// 0, 0)` when no JSONL exists for the uuid. Wraps
/// [`read_layer1_bubbles_from_body`] so the public entry point can
/// locate the file via [`paths::find_layer1_jsonl_for`] and the
/// internal parser stays file-system-agnostic (and easier to unit-test).
pub fn read_layer1_bubbles_from_path(uuid: &str, path: &Path) -> (Vec<Bubble>, usize, usize) {
    match std::fs::read_to_string(path) {
        Ok(body) => read_layer1_bubbles_from_body(uuid, &body),
        Err(_) => (Vec::new(), 0usize, 0usize),
    }
}

// ── Layer 2 reader (v0.2.2: surface store.db bubbles) ───────

/// v0.2.2: read Layer 2 (CLI `~/.cursor/chats/<md5>/<uuid>/store.db`)
/// bubbles for `uuid`.
///
/// Best-effort: walks every row in the `blobs` table and tries to
/// decode each blob's bytes as JSON. Two decode shapes are accepted:
///
/// 1. **v0.2-alpha synthesized** (what `core::sync::write_layer2`
///    writes today): `{role, text, createdAt}` — a plain JSON object.
/// 2. **Full canonical** (for future L2 writes that round-trip through
///    `Bubble`): `{role, text, tool_calls, files, createdAt, id}`.
///
/// Any blob that doesn't decode either way is silently skipped (with a
/// `tracing::warn!` at the call site). The current production corpus is
/// mostly cursor-agent's native protobuf-DAG blobs, which we don't
/// parse in v0.2.2 — those are best-effort ignored, the merge will
/// still surface them via L1/L3 if available.
///
/// `cwd` is the project working directory the CLI used to start the
/// session; it determines the chat_dir (`md5(cwd)`). Pass `""` to
/// fall back to an empty chat_dir (uncommon — most callers have a
/// real `cwd` from the session's `project_path`).
pub fn read_layer2_bubbles(uuid: &str, cwd: &str) -> Vec<Bubble> {
    let store_db = paths::store_db_for(cwd, uuid);
    if !store_db.exists() {
        return Vec::new();
    }
    let Ok(r) = storage::open_read(&store_db) else {
        return Vec::new();
    };
    let blob_ids = match r.list_blob_ids() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for (ordinal, blob_id) in blob_ids.iter().enumerate() {
        let Some(bytes) = r.get_item_binary(blob_id, "blobs").ok().flatten() else {
            continue;
        };
        if let Some(b) = decode_l2_blob(uuid, blob_id, &bytes, ordinal) {
            out.push(b);
        }
    }
    out
}

/// Attempt to decode one L2 blob as a [`Bubble`]. Returns `None` for
/// blobs that don't match either supported JSON shape (the most common
/// case being cursor-agent's native protobuf-DAG blobs).
fn decode_l2_blob(uuid: &str, blob_id: &str, bytes: &[u8], ordinal: usize) -> Option<Bubble> {
    let text = std::str::from_utf8(bytes).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;

    // Shape (a): full canonical — matches what `Bubble` looks like when
    // round-tripped through serde. The `id` field is honored when
    // present so we don't re-hash a bubble we already named.
    if let Some(b) = try_decode_canonical_bubble(&v) {
        let b = if b.id.is_empty() {
            Bubble {
                id: super::inject::deterministic_bubble_id(
                    uuid,
                    &b.role,
                    b.created_at_ms,
                    ordinal,
                ),
                ..b
            }
        } else {
            b
        };
        return Some(b);
    }

    // Shape (b): v0.2-alpha synthesized — `{role, text, createdAt}`.
    // Used by `core::sync::write_layer2`'s JSON-blob fallback (the
    // `blobs.is_empty() && !bubbles.is_empty()` branch, sync.rs:295).
    let role = v.get("role").and_then(|x| x.as_str())?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let text_body = v
        .get("text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if text_body.trim().is_empty() {
        return None;
    }
    let created_at_ms = v.get("createdAt").and_then(|x| x.as_i64()).unwrap_or(0);
    let id = if blob_id.is_empty() {
        super::inject::deterministic_bubble_id(uuid, role, created_at_ms, ordinal)
    } else {
        blob_id.to_string()
    };
    Some(Bubble {
        id,
        role: role.to_string(),
        text: text_body,
        tool_calls: Vec::new(),
        files: Vec::new(),
        created_at_ms,
        parent_bubble_id: None,
    })
}

/// Decode a JSON value as a full [`Bubble`] (Shape (a)). Returns `None`
/// when the shape is incomplete or has non-string `role`.
fn try_decode_canonical_bubble(v: &serde_json::Value) -> Option<Bubble> {
    let role = v.get("role").and_then(|x| x.as_str())?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let created_at_ms = v.get("createdAt").and_then(|x| x.as_i64()).unwrap_or(0);
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let tool_calls: Vec<BubbleToolUse> = v
        .get("tool_calls")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| {
                    let name = tc.get("name").and_then(|x| x.as_str())?.to_string();
                    let input = tc.get("input").cloned();
                    Some(BubbleToolUse { name, input })
                })
                .collect()
        })
        .unwrap_or_default();
    let files: Vec<String> = v
        .get("files")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if text.trim().is_empty() && tool_calls.is_empty() && files.is_empty() {
        return None;
    }
    Some(Bubble {
        id,
        role: role.to_string(),
        text,
        tool_calls,
        files,
        created_at_ms,
        parent_bubble_id: None,
    })
}

// ── Layer 3 reader (v0.2.2: surface state.vscdb composerData.bubbleBlobs) ──

/// v0.2.2: read Layer 3 (Cursor Desktop `state.vscdb`) bubbles for
/// `uuid`. Walks every `bubbleId:<uuid>:<bid>` row in the
/// `cursorDiskKV` table and decodes each blob as a [`Bubble`].
///
/// Cursor's native bubble blob shape (reverse-engineered from
/// `c1ea7999-…`, mirror in `inject::compose_bubble_blobs`):
/// ```json
/// { "_v": 3, "type": 1 or 2, "text": "...", ...other_fields... }
/// ```
/// `type: 1` maps to `role: "user"`, `type: 2` maps to `"assistant"`.
/// The `<bid>` portion of the row key becomes the bubble's `id` so
/// L3 entries are stable across reads (matches what Cursor uses
/// internally for diff/render).
///
/// Returns `Vec::new()` when `state.vscdb` doesn't exist or the
/// `cursorDiskKV` table is missing (e.g. on a CLI-only install).
pub fn read_layer3_bubbles(uuid: &str) -> Vec<Bubble> {
    let db_path = match paths::global_db_path() {
        Ok(p) if p.exists() => p,
        _ => return Vec::new(),
    };
    let Ok(r) = storage::open_read(&db_path) else {
        return Vec::new();
    };
    let prefix = format!("bubbleId:{uuid}:");
    let keys = match r.list_keys(&prefix, "cursorDiskKV") {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for key in keys {
        let Some(bid) = key.strip_prefix(&prefix) else {
            continue;
        };
        let Some(v) = r.get_json(&key, "cursorDiskKV").ok().flatten() else {
            continue;
        };
        if let Some(b) = decode_l3_bubble_blob(bid, &v) {
            out.push(b);
        }
    }
    // Stable sort by id so merge layer-priority is deterministic.
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

// ── L3 bubble text extraction (ported from cursor-history storage.ts) ──

/// First non-empty string param from `params` object (cursor-history `getParam`).
fn l3_get_param(params: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(s) = params.get(*key).and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

/// Parse `toolFormerData.params` (JSON string or object) or `rawArgs`.
fn l3_parse_tool_params(tool: &serde_json::Value) -> Option<serde_json::Value> {
    if let Some(p) = tool.get("params") {
        if let Some(s) = p.as_str() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                return Some(v);
            }
        }
        if p.is_object() {
            return Some(p.clone());
        }
    }
    if let Some(raw) = tool.get("rawArgs") {
        if let Some(s) = raw.as_str() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                return Some(v);
            }
        }
        if raw.is_object() {
            return Some(raw.clone());
        }
    }
    None
}

fn l3_extract_thinking_text(v: &serde_json::Value) -> Option<String> {
    v.get("thinking")
        .and_then(|t| t.get("text"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

/// Collect `codeBlocks[].content` as markdown-fenced strings when languageId is set.
fn l3_extract_code_block_parts(v: &serde_json::Value) -> Vec<String> {
    let Some(blocks) = v.get("codeBlocks").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for cb in blocks {
        let Some(content) = cb.get("content").and_then(|x| x.as_str()) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        let lang = cb
            .get("languageId")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if lang.is_empty() {
            out.push(content.to_string());
        } else {
            out.push(format!("```{lang}\n{content}\n```"));
        }
    }
    out
}

fn l3_format_diff_block(diff: &serde_json::Value) -> Option<String> {
    let chunks = diff.get("chunks")?.as_array()?;
    let parts: Vec<String> = chunks
        .iter()
        .filter_map(|c| c.get("diffString").and_then(|x| x.as_str()))
        .map(|s| s.to_string())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Tool call with `result.diff` (write / edit). Storage layer keeps full text (spec 013).
fn l3_format_tool_with_result(tool: &serde_json::Value) -> Option<String> {
    let result_str = tool.get("result").and_then(|x| x.as_str()).unwrap_or("{}");
    let result: serde_json::Value = serde_json::from_str(result_str).ok()?;
    let diff = result.get("diff")?;
    if !diff.is_object() {
        return None;
    }
    let mut lines = Vec::new();
    let name = tool.get("name").and_then(|x| x.as_str()).unwrap_or("write");
    let label = if name == "write" || name == "write_file" {
        "Write File"
    } else {
        "Edit File"
    };
    lines.push(format!("[Tool: {label}]"));
    if let Some(params) = l3_parse_tool_params(tool) {
        let file = l3_get_param(
            &params,
            &["relativeWorkspacePath", "file_path", "targetFile", "path"],
        );
        if !file.is_empty() {
            lines.push(format!("File: {file}"));
        }
    }
    if let Some(diff_text) = l3_format_diff_block(diff) {
        lines.push(String::new());
        lines.push(diff_text);
    }
    if let Some(rfm) = result.get("resultForModel").and_then(|x| x.as_str()) {
        lines.push(String::new());
        lines.push(format!("Result: {rfm}"));
    }
    if lines.len() <= 1 {
        return None;
    }
    Some(lines.join("\n"))
}

/// Format a toolFormerData bubble for display. read_file_v2 / edit_file_v2: no truncation (spec 013).
fn l3_format_tool_call(tool: &serde_json::Value, code_blocks: Option<&serde_json::Value>) -> String {
    let name = tool.get("name").and_then(|x| x.as_str()).unwrap_or("unknown");
    let params = l3_parse_tool_params(tool).unwrap_or(serde_json::json!({}));
    let mut lines = vec![format!("[Tool: {name}]")];

    match name {
        "read_file_v2" | "read_file" => {
            let file = l3_get_param(
                &params,
                &["targetFile", "path", "file", "effectiveUri"],
            );
            if !file.is_empty() {
                lines.push(format!("File: {file}"));
            }
            if let Some(result_str) = tool.get("result").and_then(|x| x.as_str()) {
                if let Ok(result) = serde_json::from_str::<serde_json::Value>(result_str) {
                    if let Some(contents) = result.get("contents").and_then(|x| x.as_str()) {
                        lines.push(format!("Content: {contents}"));
                    }
                    if let Some(diff) = result.get("diff") {
                        if let Some(diff_text) = l3_format_diff_block(diff) {
                            lines.push(diff_text);
                        }
                    }
                }
            }
            if let Some(blocks) = code_blocks.and_then(|x| x.as_array()) {
                if let Some(content) = blocks
                    .first()
                    .and_then(|b| b.get("content"))
                    .and_then(|x| x.as_str())
                {
                    if !content.is_empty() && !lines.iter().any(|l| l.starts_with("Content:")) {
                        lines.push(format!("Content: {content}"));
                    }
                }
            }
        }
        "edit_file_v2" | "edit_file" | "search_replace" => {
            let file = l3_get_param(
                &params,
                &["targetFile", "path", "file", "relativeWorkspacePath"],
            );
            if !file.is_empty() {
                lines.push(format!("File: {file}"));
            }
            if let Some(result_str) = tool.get("result").and_then(|x| x.as_str()) {
                if let Ok(result) = serde_json::from_str::<serde_json::Value>(result_str) {
                    if let Some(diff) = result.get("diff") {
                        if let Some(diff_text) = l3_format_diff_block(diff) {
                            lines.push(diff_text);
                        }
                    }
                } else if !result_str.is_empty() {
                    lines.push(result_str.to_string());
                }
            }
        }
        "run_terminal_command" | "run_terminal_cmd" | "execute_command" => {
            let cmd = l3_get_param(&params, &["command", "cmd"]);
            if !cmd.is_empty() {
                lines.push(format!("Command: {cmd}"));
            }
            if let Some(result_str) = tool.get("result").and_then(|x| x.as_str()) {
                if let Ok(result) = serde_json::from_str::<serde_json::Value>(result_str) {
                    if let Some(output) = result.get("output").and_then(|x| x.as_str()) {
                        if !output.trim().is_empty() {
                            lines.push(format!("Output: {output}"));
                        }
                    }
                }
            }
        }
        "grep" | "search" | "codebase_search" => {
            let pattern = l3_get_param(&params, &["pattern", "query", "searchQuery", "regex"]);
            let path = l3_get_param(&params, &["path", "directory", "targetDirectory"]);
            if !pattern.is_empty() {
                lines.push(format!("Pattern: {pattern}"));
            }
            if !path.is_empty() {
                lines.push(format!("Path: {path}"));
            }
        }
        "list_dir" => {
            let dir = l3_get_param(&params, &["targetDirectory", "path", "directory"]);
            if !dir.is_empty() {
                lines.push(format!("Directory: {dir}"));
            }
        }
        _ => {
            if !params.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                if let Ok(s) = serde_json::to_string_pretty(&params) {
                    lines.push(s);
                }
            }
            if let Some(result_str) = tool.get("result").and_then(|x| x.as_str()) {
                if !result_str.trim().is_empty() {
                    lines.push(result_str.to_string());
                }
            }
        }
    }

    lines.join("\n")
}

/// Structured tool calls for UI (`BubbleToolUse`). Mirrors cursor-history `extractToolCalls`.
fn l3_extract_tool_calls(v: &serde_json::Value) -> Vec<BubbleToolUse> {
    let Some(tool) = v.get("toolFormerData") else {
        return Vec::new();
    };
    let Some(name) = tool.get("name").and_then(|x| x.as_str()) else {
        return Vec::new();
    };
    if name.trim().is_empty() {
        return Vec::new();
    }
    let input = l3_parse_tool_params(tool);
    vec![BubbleToolUse {
        name: name.to_string(),
        input,
    }]
}

fn l3_extract_tool_files(v: &serde_json::Value) -> Vec<String> {
    let Some(tool) = v.get("toolFormerData") else {
        return Vec::new();
    };
    let Some(params) = l3_parse_tool_params(tool) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for key in [
        "targetFile",
        "file",
        "filePath",
        "relativeWorkspacePath",
        "path",
        "targetDirectory",
        "directory",
    ] {
        let f = l3_get_param(&params, &[key]);
        if !f.is_empty() && !files.contains(&f) {
            files.push(f);
        }
    }
    files
}

/// Multi-source text extraction for L3 `bubbleId:*` blobs (cursor-history `extractBubbleText`).
fn extract_l3_bubble_text(v: &serde_json::Value, is_assistant: bool) -> String {
    let code_block_parts = l3_extract_code_block_parts(v);
    let tool_former = v.get("toolFormerData");

    if let Some(tool) = tool_former {
        let tool_name = tool.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if tool_name != "read_file_v2" {
            if let Some(text) = l3_format_tool_with_result(tool) {
                return text;
            }
        }
        if !tool_name.is_empty() {
            return l3_format_tool_call(tool, v.get("codeBlocks"));
        }
    }

    if is_assistant {
        if let Some(text) = v.get("text").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty())
        {
            if code_block_parts.is_empty() {
                return text.to_string();
            }
            return format!("{}\n\n{}", text, code_block_parts.join("\n\n"));
        }
        if let Some(thinking) = l3_extract_thinking_text(v) {
            if code_block_parts.is_empty() {
                return format!("[Thinking]\n{thinking}");
            }
            return format!(
                "[Thinking]\n{thinking}\n\n{}",
                code_block_parts.join("\n\n")
            );
        }
        if let Some(tool) = tool_former {
            if let Some(result_str) = tool.get("result").and_then(|x| x.as_str()) {
                if let Ok(result) = serde_json::from_str::<serde_json::Value>(result_str) {
                    for key in ["contents", "content", "text"] {
                        if let Some(s) = result.get(key).and_then(|x| x.as_str()) {
                            return s.to_string();
                        }
                    }
                } else if result_str.len() > 50 && !result_str.starts_with('{') {
                    return result_str.to_string();
                }
            }
        }
        if !code_block_parts.is_empty() {
            return code_block_parts.join("\n\n");
        }
    } else {
        if !code_block_parts.is_empty() {
            return code_block_parts.join("\n\n");
        }
        for key in [
            "text",
            "content",
            "finalText",
            "message",
            "markdown",
            "textDescription",
        ] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty())
            {
                return s.to_string();
            }
        }
        if let Some(thinking) = l3_extract_thinking_text(v) {
            return format!("[Thinking]\n{thinking}");
        }
    }

    v.get("text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// Timestamp fallback chain (cursor-history spec 010, simplified).
fn extract_l3_timestamp(v: &serde_json::Value) -> i64 {
    if let Some(t) = v.get("createdAt").and_then(|x| x.as_i64()) {
        if t > 0 {
            return t;
        }
    }
    if let Some(ti) = v.get("timingInfo") {
        for key in ["clientRpcSendTime", "clientStartTime", "clientEndTime"] {
            if let Some(t) = ti.get(key).and_then(|x| x.as_i64()) {
                if t > 0 {
                    return t;
                }
            }
        }
    }
    0
}

/// Decode one L3 `bubbleId:*` blob as a [`Bubble`]. Returns `None` when
/// the blob doesn't have the expected `_v`/`type`/`text` triple or
/// `type` is something other than 1/2.
fn decode_l3_bubble_blob(bid: &str, v: &serde_json::Value) -> Option<Bubble> {
    let typ = v.get("type").and_then(|x| x.as_i64()).unwrap_or(0);
    let role = match typ {
        1 => "user",
        2 => "assistant",
        _ => return None,
    };
    let is_assistant = typ == 2;
    let text = extract_l3_bubble_text(v, is_assistant);
    let tool_calls = l3_extract_tool_calls(v);
    let files = l3_extract_tool_files(v);
    let created_at_ms = extract_l3_timestamp(v);
    Some(Bubble {
        id: bid.to_string(),
        role: role.to_string(),
        text,
        tool_calls,
        files,
        created_at_ms,
        parent_bubble_id: None,
    })
}

// ── 3-way merge (v0.2.2: bubble-id reconciliation) ───────────

/// v0.2.2: reconcile bubbles from L1 (JSONL), L2 (store.db), and L3
/// (state.vscdb) into a single ordered list.
///
/// Algorithm:
///   1. L3 bubbles form the **main chain** (they're what Cursor Desktop
///      actually shows users — most authoritative).
///   2. L2 bubbles are overlaid by `id`: when an L2 bubble's id matches
///      an L3 bubble, L2 fields take precedence **only when L2's value
///      is non-empty / non-zero** (LWW by non-empty). This handles the
///      common case where `core::sync::write_layer2` re-wrote a blob
///      after L3 had it.
///   3. L1 bubbles participate by their synthetic id
///      (`deterministic_bubble_id(uuid, role, ts, ordinal)`) and act as
///      **fallback source**: an L1 bubble is included only when neither
///      L3 nor L2 has a bubble with the same id. L1 is the most
///      lossy layer (it can't carry tool-call structure), so we treat
///      it as a backstop, not a leader.
///   4. Final ordering: `(created_at_ms ASC, id ASC)`. L1/L2 bubbles
///      with `created_at_ms == 0` (the common case for JSONL) sort to
///      the front but are still differentiated by their synthetic id.
///
/// Duplicate ids across L1+L2+L3 collapse to one row; duplicate text
/// within a single layer is preserved (we don't dedupe by content).
pub fn merge_bubbles_three_way(
    l1: Vec<Bubble>,
    l2: Vec<Bubble>,
    l3: Vec<Bubble>,
) -> Vec<Bubble> {
    use std::collections::HashMap;

    // 1. Index L3 by id (main chain).
    let mut by_id: HashMap<String, Bubble> = HashMap::with_capacity(l3.len());
    for b in l3 {
        by_id.insert(b.id.clone(), b);
    }

    // 2. Overlay L2 onto L3 by id (LWW by non-empty field).
    for b in l2 {
        merge_into(&mut by_id, b);
    }

    // 3. L1 fallback: only insert bubbles whose id is absent.
    for b in l1 {
        if !by_id.contains_key(&b.id) {
            by_id.insert(b.id.clone(), b);
        }
    }

    // 4. Sort by (created_at_ms ASC, id ASC).
    let mut out: Vec<Bubble> = by_id.into_values().collect();
    out.sort_by(|a, b| {
        a.created_at_ms
            .cmp(&b.created_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Field-level LWW merge of `incoming` into `existing` (looked up in
/// `by_id` by `incoming.id`). A field on `incoming` overwrites the
/// existing value **only when** it carries data (non-empty string /
/// non-zero integer / non-empty vec). This lets a "downgrade" from a
/// richer L3 bubble to a degraded v0.2-alpha L2 blob (which only has
/// `{role, text, createdAt}`) leave the L3 fields like `tool_calls`
/// intact instead of blanking them.
fn merge_into(by_id: &mut std::collections::HashMap<String, Bubble>, incoming: Bubble) {
    let key = incoming.id.clone();
    let existing = match by_id.get_mut(&key) {
        Some(e) => e,
        None => {
            by_id.insert(key, incoming);
            return;
        }
    };
    if !incoming.text.is_empty() {
        existing.text = incoming.text;
    }
    if !incoming.role.is_empty() && (existing.role.is_empty() || existing.role == "assistant") {
        // Only override role when existing is empty/ambiguous — don't
        // ever let a degraded blob flip a known role.
        existing.role = incoming.role;
    }
    if incoming.created_at_ms != 0 {
        existing.created_at_ms = incoming.created_at_ms;
    }
    if !incoming.tool_calls.is_empty() {
        existing.tool_calls = incoming.tool_calls;
    }
    if !incoming.files.is_empty() {
        existing.files = incoming.files;
    }
}

/// Strip wrapper tags (`<user_query>…</user_query>`, `<timestamp>…</timestamp>`)
/// from a user bubble but **keep** all lines and newlines (full-text variant).
fn clean_user_text(text: &str) -> String {
    let mut s = text.trim().to_string();
    if s.starts_with("<timestamp>") {
        if let Some(idx) = s.find("</timestamp>") {
            s = s[idx + "</timestamp>".len()..].trim().to_string();
        }
    }
    let open = "<user_query>";
    let close = "</user_query>";
    if s.starts_with(open) {
        s = s[open.len()..].to_string();
    }
    if s.ends_with(close) {
        s = s[..s.len() - close.len()].to_string();
    }
    s.trim().to_string()
}

// ── Full-content index snippet ──────────────────────────────
//
// Used for cross-conversation search on the frontend. Reads each JSONL
// once (already cached in OS page cache by the title pass for most files)
// and joins the first ~2 KB of clean text content. Capping per-session
// keeps the wire payload bounded even for very long transcripts
// (37 sessions × 2 KB ≈ 75 KB total).

const INDEXABLE_MAX_CHARS: usize = 2000;

/// Pull a contiguous text snippet (≤ INDEXABLE_MAX_CHARS) from a JSONL file.
///
/// Like `read_jsonl_preview`, it understands both the nested schema
/// (`v.message.content[].text`) and the legacy top-level `content`. It
/// stops as soon as the buffer is full, so very large transcripts are
/// not fully scanned.
fn read_jsonl_indexable(path: &Path) -> String {
    let Ok(body) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut buf = String::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("user");

        let content_arr = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .or_else(|| v.get("content").and_then(|c| c.as_array()));

        if let Some(arr) = content_arr {
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) != Some("text") {
                    continue;
                }
                if let Some(text) = item.get("text").and_then(|x| x.as_str()) {
                    let cleaned = if role == "user" {
                        clean_user_text(text)
                    } else {
                        text.to_string()
                    };
                    if cleaned.is_empty() {
                        continue;
                    }
                    let sep = if buf.is_empty() { 0 } else { 1 };
                    if buf.len() + sep + cleaned.len() > INDEXABLE_MAX_CHARS {
                        let room = INDEXABLE_MAX_CHARS.saturating_sub(buf.len() + sep);
                        if room > 0 {
                            buf.push_str(&truncate_to_char_boundary(&cleaned, room));
                        }
                        return buf;
                    }
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(&cleaned);
                }
            }
        } else if let Some(raw) = v.get("content").and_then(|c| c.as_str()) {
            let cleaned = if role == "user" {
                clean_user_text(raw)
            } else {
                raw.to_string()
            };
            if !cleaned.is_empty() {
                let sep = if buf.is_empty() { 0 } else { 1 };
                if buf.len() + sep + cleaned.len() > INDEXABLE_MAX_CHARS {
                    let room = INDEXABLE_MAX_CHARS.saturating_sub(buf.len() + sep);
                    if room > 0 {
                        buf.push_str(&truncate_to_char_boundary(&cleaned, room));
                    }
                    return buf;
                }
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&cleaned);
            }
        }
    }
    buf
}

/// Return the largest prefix of `s` whose UTF-8 length does not exceed
/// `max_bytes`. Walks back to the previous char boundary so we never
/// slice into a multi-byte character (the crash the unit-test fix
/// addresses).
fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut idx = max_bytes.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    s[..idx].to_string()
}

// ── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, body: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("bc-test-{name}-{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    /// Real cursor-agent JSONL: `{role, message: {content: [{type, text}]}}`
    /// (verified 2026/07 against `~/.cursor/projects/.../agent-transcripts/`).
    #[test]
    fn parse_nested_message_content() {
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\n你现在用的是什么模型?\n</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"I'm Composer."},{"type":"tool_use","name":"Glob","input":{}}]}}
{"role":"user","message":{"content":[{"type":"text","text":"our sessions stored where?"}]}}
"#;
        let p = write_tmp("nested", body);
        let (preview, _files) = read_jsonl_preview(&p);
        // First user line, stripped of <user_query> wrappers.
        assert_eq!(preview, "你现在用的是什么模型?");
    }

    /// Legacy schema: content directly on root, may be a string.
    #[test]
    fn parse_root_string_content() {
        let body = r#"{"role":"user","content":"hello legacy"}
{"role":"assistant","content":"hi"}
"#;
        let p = write_tmp("root-string", body);
        let (preview, _files) = read_jsonl_preview(&p);
        assert_eq!(preview, "hello legacy");
    }

    /// Legacy schema: content array of parts on root.
    #[test]
    fn parse_root_array_content() {
        let body = r#"{"role":"user","content":[{"type":"text","text":"hi from array"}]}
"#;
        let p = write_tmp("root-array", body);
        let (preview, _files) = read_jsonl_preview(&p);
        assert_eq!(preview, "hi from array");
    }

    /// Skips lines with no role:user (e.g. turn_ended / error events).
    #[test]
    fn parse_skips_orphan_events() {
        let body = r#"{"type":"turn_ended","status":"error","error":"usage limit"}
{"role":"user","message":{"content":[{"type":"text","text":"<user_query>the real question</user_query>"}]}}
"#;
        let p = write_tmp("orphan", body);
        let (preview, _files) = read_jsonl_preview(&p);
        assert_eq!(preview, "the real question");
    }

    /// First user line is truncated to 120 chars + ellipsis.
    #[test]
    fn parse_caps_preview_at_120() {
        let big = "X".repeat(300);
        let body = format!(
            r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"{big}"}}]}}}}"#,
        );
        let p = write_tmp("cap", &body);
        let (preview, _files) = read_jsonl_preview(&p);
        assert_eq!(preview.chars().count(), 121); // 120 + '…'
        assert!(preview.ends_with('…'));
    }

    /// <timestamp>...</timestamp> block before <user_query> is dropped.
    #[test]
    fn parse_strips_timestamp_block() {
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>2026-07-03T10:00:00Z</timestamp>\n<user_query>\nreal q\n</user_query>"}]}}
"#;
        let p = write_tmp("timestamp", body);
        let (preview, _files) = read_jsonl_preview(&p);
        assert_eq!(preview, "real q");
    }

    /// Returns empty (no user line at all) → UI falls back to "Untitled · …".
    #[test]
    fn parse_no_user_message_yields_empty() {
        let body = r#"{"type":"turn_ended","status":"error","error":"usage limit"}
{"role":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}
"#;
        let p = write_tmp("no-user", body);
        let (preview, _files) = read_jsonl_preview(&p);
        assert_eq!(preview, "");
    }

    // ─── Conversation / bubbles ───────────────────────────────

    #[test]
    fn conv_user_then_assistant_with_tool_use() {
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nhi\n</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"**hi** back"},{"type":"tool_use","name":"Glob","input":{"pattern":"*"}},{"type":"tool_use","name":"Read","input":{"path":"/etc/hostname"}}]}}
"#;
        let dir = std::env::temp_dir().join(format!("bc-conv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl_path = dir.join("abc.jsonl");
        std::fs::write(&jsonl_path, body).unwrap();

        let conv = read_conversation_from_path("abc", &jsonl_path);
        assert!(conv.source_path.is_some());
        assert_eq!(conv.bubbles.len(), 2);
        assert_eq!(conv.bubbles[0].role, "user");
        assert_eq!(conv.bubbles[0].text, "hi");
        assert_eq!(conv.bubbles[1].role, "assistant");
        assert_eq!(conv.bubbles[1].text, "**hi** back");
        assert_eq!(conv.bubbles[1].tool_calls.len(), 2);
        assert_eq!(conv.bubbles[1].tool_calls[0].name, "Glob");
        assert_eq!(
            conv.bubbles[1].tool_calls[0].input.as_ref().unwrap()["pattern"],
            "*"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conv_orphan_events_dropped() {
        let body = r#"{"type":"turn_ended","status":"error","error":"usage limit"}
{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nq\n</user_query>"}]}}
"#;
        let dir = std::env::temp_dir().join(format!("bc-conv-orphan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl_path = dir.join("zzz.jsonl");
        std::fs::write(&jsonl_path, body).unwrap();

        let conv = read_conversation_from_path("zzz", &jsonl_path);
        assert_eq!(conv.bubbles.len(), 1);
        assert_eq!(conv.bubbles[0].role, "user");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conv_empty_string_content_drops_bubble() {
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":""}]}}
{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\ngenuine\n</user_query>"}]}}
"#;
        let dir = std::env::temp_dir().join(format!("bc-conv-empt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl_path = dir.join("ww.jsonl");
        std::fs::write(&jsonl_path, body).unwrap();

        let conv = read_conversation_from_path("ww", &jsonl_path);
        assert_eq!(conv.bubbles.len(), 1);
        assert_eq!(conv.bubbles[0].text, "genuine");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conv_missing_uuid_returns_empty() {
        let conv = read_conversation("definitely-not-on-disk-12345678");
        assert!(conv.source_path.is_none());
        assert!(conv.bubbles.is_empty());
    }

    // ─── Indexable text (full-content search snippet) ────────

    #[test]
    fn indexable_joins_user_and_assistant_text() {
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nfind me a graphql server\n</user_query>"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"Try yoga or mercurius"},{"type":"tool_use","name":"Glob","input":{}}]}}
"#;
        let p = write_tmp("idx-join", body);
        let text = read_jsonl_indexable(&p);
        assert!(text.contains("graphql server"), "got: {text:?}");
        assert!(text.contains("yoga or mercurius"), "got: {text:?}");
        assert!(!text.contains("<user_query>"), "wrappers leaked: {text:?}");
    }

    #[test]
    fn indexable_caps_at_max() {
        let big = "X".repeat(5000);
        let body = format!(
            r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"{big}"}}]}}}}"#,
        );
        let p = write_tmp("idx-cap", &body);
        let text = read_jsonl_indexable(&p);
        // ASCII so byte count == char count == cap.
        assert_eq!(text.len(), INDEXABLE_MAX_CHARS);
    }

    /// The actual bug that crashed Tauri before fix: a 3-byte UTF-8 char
    /// at a byte index ≤ INDEXABLE_MAX_CHARS used to fall partially into
    /// `String::truncate` and panic. Now we truncate at the previous
    /// char boundary, so the result must be ≤ INDEXABLE_MAX_CHARS
    /// *and* end on a clean char boundary.
    #[test]
    fn indexable_does_not_panic_on_multibyte() {
        // 3000 × "中" = 3 bytes each = 9000 bytes; well over the 2 KB cap.
        let cjk = "中".repeat(3000);
        let body = format!(
            r#"{{"role":"user","message":{{"content":[{{"type":"text","text":"{cjk}"}}]}}}}"#,
        );
        let p = write_tmp("idx-cjk", &body);
        let text = read_jsonl_indexable(&p);
        assert!(text.len() <= INDEXABLE_MAX_CHARS);
        assert!(text.is_char_boundary(text.len()), "not on char boundary");
        // Whole-char count × 3 should still leave us under the cap.
        let char_count = text.chars().count();
        assert!(char_count * 3 <= INDEXABLE_MAX_CHARS);
    }

    #[test]
    fn indexable_skips_tool_use_only_messages() {
        let body = r#"{"role":"assistant","message":{"content":[{"type":"tool_use","name":"Shell","input":{}}]}}
{"role":"user","message":{"content":[{"type":"text","text":"<user_query>\nreal query\n</user_query>"}]}}
"#;
        let p = write_tmp("idx-no-tool", body);
        let text = read_jsonl_indexable(&p);
        assert_eq!(text, "real query");
    }

    #[test]
    fn indexable_empty_on_missing_file() {
        let text = read_jsonl_indexable(Path::new("/nope/does-not-exist.jsonl"));
        assert!(text.is_empty());
    }

    // ─── Broken-session detection (Layer 2 latestRootBlobId) ──

    /// Pure-JSON half of `read_store_db_meta` so we can unit-test the
    /// "empty string root blob" detection without spinning up SQLite.
    fn parse_store_meta_json(v: &serde_json::Value) -> (String, i64, Option<String>) {
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let created_at = v.get("createdAt").and_then(|x| x.as_i64()).unwrap_or(0);
        let root = v
            .get("latestRootBlobId")
            .and_then(|x| x.as_str())
            .map(str::to_string);
        (name, created_at, root)
    }

    #[test]
    fn broken_rule_fires_on_empty_root_blob() {
        // Simulate the cursor-agent data-loss case `c1ea7999` had:
        // store.db has blobs but meta.latestRootBlobId == "".
        let meta = serde_json::json!({
            "name": "session",
            "createdAt": 100,
            "latestRootBlobId": ""
        });
        let (name, _ts, root) = parse_store_meta_json(&meta);
        assert_eq!(name, "session");
        assert_eq!(root.as_deref(), Some(""));
        // ← This is the condition that should set `is_broken` upstream.
        assert!(matches!(root.as_deref(), Some(s) if s.is_empty()));
    }

    #[test]
    fn broken_rule_does_not_fire_on_valid_root_blob() {
        let meta = serde_json::json!({
            "name": "session",
            "createdAt": 100,
            "latestRootBlobId": "abc123def"
        });
        let (_name, _ts, root) = parse_store_meta_json(&meta);
        assert!(!matches!(root.as_deref(), Some(s) if s.is_empty()));
    }

    #[test]
    fn broken_rule_does_not_fire_on_missing_root_blob() {
        // Layer 1-only sessions have no meta at all.
        let meta = serde_json::json!({
            "name": "session",
            "createdAt": 100
        });
        let (_name, _ts, root) = parse_store_meta_json(&meta);
        assert!(root.is_none());
    }

    /// Live diagnostic: print every session's source-layer composition.
    /// Used to verify Layer 1-only sessions are NOT mistakenly tagged
    /// linux_cli just because a JSONL exists (see #87). Run with:
    ///   `cargo test --lib canonical::tests::diag_print_all_sessions -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn diag_print_all_sessions() {
        let sessions = crate::core::canonical::scan_all().expect("scan_all");
        eprintln!("uuid (8) | sources                          | L3 | project");
        eprintln!("---------+----------------------------------+----+--------");
        for s in &sessions {
            let mut bits = Vec::new();
            if let Some(info) = &s.sources.linux_cli {
                bits.push(format!("linux_cli:L{}", info.layer));
            }
            if let Some(info) = &s.sources.linux_desktop {
                bits.push(format!("linux_desktop:L{}", info.layer));
            }
            if let Some(info) = &s.sources.mac {
                bits.push(format!("mac:L{}", info.layer));
            }
            let summary = if bits.is_empty() { "(none!)".to_string() } else { bits.join(" + ") };
            eprintln!("{} | {:32} | {} | {}",
                &s.uuid[..8.min(s.uuid.len())],
                summary,
                if s.layer_3_present { "✓" } else { "·" },
                &s.project_slug);
        }
    }

    #[test]
    fn extract_workspace_path_handles_object_form() {
        // Real Cursor writes workspaceIdentifier as an object with a
        // nested uri. Before this helper existed, scan_layer3_into
        // tried `.as_str()` on the object and got an empty string,
        // falling through to `desktop-<uuid>` slug (#88).
        let v = serde_json::json!({
            "workspaceIdentifier": {
                "id": "a2a619b49a9779a3952a95ce4c9579bf",
                "uri": {
                    "fsPath": "/home/eric/workspace/enenzuo",
                    "external": "file:///home/eric/workspace/enenzuo",
                    "path": "/home/eric/workspace/enenzuo",
                    "scheme": "file"
                }
            }
        });
        assert_eq!(extract_workspace_path(&v), "/home/eric/workspace/enenzuo");
    }

    #[test]
    fn extract_workspace_path_handles_legacy_string_form() {
        // Older / synthetic entries may use the bare-string shape.
        let v = serde_json::json!({"workspaceIdentifier": "/Users/x/y"});
        assert_eq!(extract_workspace_path(&v), "/Users/x/y");
    }

    #[test]
    fn extract_workspace_path_handles_missing_field() {
        let v = serde_json::json!({"name": "no workspace"});
        assert_eq!(extract_workspace_path(&v), "");
        let v2 = serde_json::json!({"workspaceIdentifier": null});
        assert_eq!(extract_workspace_path(&v2), "");
    }

    #[test]
    fn extract_workspace_path_strips_file_scheme() {
        // When only `external` is present, must strip `file://`.
        let v = serde_json::json!({
            "workspaceIdentifier": {
                "uri": {"external": "file:///foo/bar"}
            }
        });
        assert_eq!(extract_workspace_path(&v), "/foo/bar");
    }

    /// #102: `last_updated_at` for Layer 3 sessions must come from
    /// the per-session JSON `lastUpdatedAt` field, NOT from the shared
    /// state.vscdb mtime. Otherwise every Layer 3 session in a single
    /// refresh shares the same `last_updated_at` (the file mtime at
    /// scan time), and the sidebar order degenerates to insertion
    /// order — looking like the rows are "jumping around" any time a
    /// write touches state.vscdb.
    ///
    /// We exercise this by feeding `merge_source` two Layer-3-style
    /// payloads with deliberately different `lastUpdatedAt` values
    /// but the same `last_seen_at` mtime. The merged entries must
    /// preserve the per-session timestamps.
    #[test]
    fn layer3_last_updated_at_uses_json_field_not_mtime() {
        let mut by_uuid: HashMap<String, CanonicalSession> = HashMap::new();
        let mtime = 1_700_000_000_000_i64; // shared mtime — same for both
        let a_updated = 1_700_000_500_000_i64; // A is 500s newer
        let b_updated = 1_700_000_100_000_i64; // B is 100s newer
        let a_created = 1_699_999_000_000_i64;
        let b_created = 1_699_998_500_000_i64;
        // Simulate scan_layer3_into's merge_source call signature:
        // last_updated_at comes from the JSON (lastUpdatedAt.max(createdAt)),
        // NOT from mtime.
        merge_source(
            &mut by_uuid,
            "uuid-a",
            SourceInfo { last_seen_at: mtime, layer: "3".into(), path: "state.vscdb".into() },
            SourceLayer::LinuxDesktop,
            "no-workspace",
            "A".into(),
            a_updated.max(a_created),
            String::new(),
        );
        merge_source(
            &mut by_uuid,
            "uuid-b",
            SourceInfo { last_seen_at: mtime, layer: "3".into(), path: "state.vscdb".into() },
            SourceLayer::LinuxDesktop,
            "no-workspace",
            "B".into(),
            b_updated.max(b_created),
            String::new(),
        );
        let a = by_uuid.get("uuid-a").unwrap();
        let b = by_uuid.get("uuid-b").unwrap();
        assert_eq!(a.last_updated_at, a_updated, "A must keep its JSON lastUpdatedAt");
        assert_eq!(b.last_updated_at, b_updated, "B must keep its JSON lastUpdatedAt");
        assert!(a.last_updated_at > b.last_updated_at);
        // The bug (pre-#102) would have both equal to `mtime`, defeating
        // the sidebar's "updated_desc" sort. Explicitly assert they differ.
        assert_ne!(a.last_updated_at, b.last_updated_at);
    }

    /// When JSON has no `lastUpdatedAt`, fall back to `createdAt`
    /// (matches the pre-#102 behavior for old/malformed rows). This
    /// guarantees we never fall back to the misleading shared mtime.
    #[test]
    fn layer3_last_updated_at_falls_back_to_created_at() {
        let mut by_uuid: HashMap<String, CanonicalSession> = HashMap::new();
        let mtime = 1_700_000_000_000_i64;
        let created = 1_699_999_000_000_i64;
        // Pre-#102 callers might have called merge_source with
        // `mtime.max(created_at)`. After #102 we expect callers to
        // pass `json_last_updated_at.max(created_at)` instead. If
        // json_last_updated_at is 0 (missing), the result must equal
        // `created`, NOT `mtime`.
        merge_source(
            &mut by_uuid,
            "uuid-x",
            SourceInfo { last_seen_at: mtime, layer: "3".into(), path: "state.vscdb".into() },
            SourceLayer::LinuxDesktop,
            "no-workspace",
            "X".into(),
            0_i64.max(created), // mirrors the new code path
            String::new(),
        );
        let x = by_uuid.get("uuid-x").unwrap();
        assert_eq!(x.last_updated_at, created);
        assert_ne!(x.last_updated_at, mtime, "must NOT fall back to global mtime");
    }

    // ─── v0.2.2: 3-layer bubble merge tests ─────────────────

    /// L1 bubbles (parsed from JSONL) must carry a stable 36-char
    /// GUID. Same input parsed twice → same id (deterministic).
    #[test]
    fn read_layer1_assigns_deterministic_ids() {
        let body = r#"{"role":"user","message":{"content":[{"type":"text","text":"hi"}]}}
{"role":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}
"#;
        let (a, _tl, _pe) = read_layer1_bubbles_from_body("uuid-x", body);
        let (b, _tl, _pe) = read_layer1_bubbles_from_body("uuid-x", body);
        assert_eq!(a.len(), 2);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.id, y.id, "id must be deterministic across calls");
            assert_eq!(x.id.len(), 36);
            assert_eq!(x.id.chars().filter(|c| *c == '-').count(), 4);
        }
        // Different uuids → different ids even at the same ordinal.
        let (c, _, _) = read_layer1_bubbles_from_body("uuid-y", body);
        assert_ne!(a[0].id, c[0].id);
    }

    /// v0.2-alpha degraded blob: `{role, text, createdAt}` should decode
    /// successfully into a `Bubble`. Mirrors what
    /// `core::sync::write_layer2`'s JSON-blob fallback writes today
    /// (sync.rs:295-323).
    #[test]
    fn decode_l2_blob_handles_v0_2_alpha_degraded_shape() {
        let json = serde_json::json!({
            "role": "user",
            "text": "hello world",
            "createdAt": 1_700_000_000_000_i64,
        })
        .to_string();
        let b = decode_l2_blob("uuid-a", "blob-1", json.as_bytes(), 0)
            .expect("should decode degraded shape");
        assert_eq!(b.role, "user");
        assert_eq!(b.text, "hello world");
        assert_eq!(b.created_at_ms, 1_700_000_000_000);
        assert!(!b.id.is_empty(), "id must be filled even on degraded blob");
    }

    /// Full canonical shape: `{role, text, tool_calls, files, createdAt, id}`.
    /// Every field round-trips through `try_decode_canonical_bubble`.
    #[test]
    fn decode_l2_blob_handles_full_canonical_shape() {
        let json = serde_json::json!({
            "role": "assistant",
            "text": "checking...",
            "tool_calls": [{"name": "Glob", "input": {"pattern": "*.rs"}}],
            "files": ["src/main.rs"],
            "createdAt": 42_i64,
            "id": "explicit-bubble-id",
        })
        .to_string();
        let b = decode_l2_blob("uuid-a", "blob-1", json.as_bytes(), 0).unwrap();
        assert_eq!(b.role, "assistant");
        assert_eq!(b.text, "checking...");
        assert_eq!(b.tool_calls.len(), 1);
        assert_eq!(b.tool_calls[0].name, "Glob");
        assert_eq!(b.files, vec!["src/main.rs".to_string()]);
        assert_eq!(b.created_at_ms, 42);
        assert_eq!(b.id, "explicit-bubble-id", "id from JSON wins");
    }

    /// Garbage bytes (not UTF-8, or non-JSON) must skip silently
    /// rather than panicking. This is the dominant case in practice
    /// (cursor-agent's native L2 blobs are protobuf, not JSON).
    #[test]
    fn decode_l2_blob_skips_garbage_gracefully() {
        // Random bytes that aren't valid UTF-8 in a meaningful way.
        let garbage = &[0x00, 0xFF, 0xAB, 0xCD, 0xEF];
        let r = decode_l2_blob("uuid-a", "blob-1", garbage, 0);
        assert!(r.is_none(), "must skip non-UTF-8 blob");

        // Valid UTF-8 but not JSON.
        let not_json = b"this is plain text, not JSON";
        let r = decode_l2_blob("uuid-a", "blob-1", not_json, 0);
        assert!(r.is_none(), "must skip non-JSON blob");

        // JSON but no `role` field.
        let no_role = serde_json::json!({"text": "orphan", "createdAt": 1}).to_string();
        let r = decode_l2_blob("uuid-a", "blob-1", no_role.as_bytes(), 0);
        assert!(r.is_none(), "must skip blob without role");

        // JSON with non-user/assistant role (system events etc).
        let system_role = serde_json::json!({"role": "system", "text": "x"}).to_string();
        let r = decode_l2_blob("uuid-a", "blob-1", system_role.as_bytes(), 0);
        assert!(r.is_none(), "must skip blob with role != user|assistant");
    }

    /// L3 decode: `{_v, type, text}` from a `bubbleId:*` row.
    /// `type: 1` → user, `type: 2` → assistant.
    #[test]
    fn decode_l3_bubble_blob_handles_known_types() {
        let user_v = serde_json::json!({
            "_v": 3,
            "type": 1,
            "text": "question",
        });
        let b = decode_l3_bubble_blob("bid-1", &user_v).unwrap();
        assert_eq!(b.role, "user");
        assert_eq!(b.text, "question");
        assert_eq!(b.id, "bid-1");

        let asst_v = serde_json::json!({
            "_v": 3,
            "type": 2,
            "text": "answer",
        });
        let b = decode_l3_bubble_blob("bid-2", &asst_v).unwrap();
        assert_eq!(b.role, "assistant");
        assert_eq!(b.text, "answer");
        assert_eq!(b.id, "bid-2");

        // Unknown type (e.g. tool result 3) → skip.
        let tool_v = serde_json::json!({"_v": 3, "type": 3, "text": "x"});
        assert!(decode_l3_bubble_blob("bid-3", &tool_v).is_none());
    }

    /// L3 toolFormerData: read_file_v2 keeps full content (spec 013, no storage-layer truncate).
    #[test]
    fn decode_l3_bubble_blob_read_file_v2_full_content() {
        let v = serde_json::json!({
            "_v": 3,
            "type": 2,
            "toolFormerData": {
                "name": "read_file_v2",
                "params": r#"{"targetFile":"src/main.rs"}"#,
                "result": r#"{"contents":"fn main() {\n    println!(\"hi\");\n}"}"#
            }
        });
        let b = decode_l3_bubble_blob("bid-rf", &v).unwrap();
        assert_eq!(b.role, "assistant");
        assert!(b.text.contains("[Tool: read_file_v2]"));
        assert!(b.text.contains("src/main.rs"));
        assert!(b.text.contains("fn main()"));
        assert_eq!(b.tool_calls.len(), 1);
        assert_eq!(b.tool_calls[0].name, "read_file_v2");
    }

    /// L3 assistant: text + codeBlocks combined; thinking fallback when text empty.
    #[test]
    fn decode_l3_bubble_blob_thinking_and_code_blocks() {
        let v = serde_json::json!({
            "_v": 3,
            "type": 2,
            "thinking": { "text": "reasoning step" },
            "codeBlocks": [{ "content": "print(1)", "languageId": "python" }]
        });
        let b = decode_l3_bubble_blob("bid-th", &v).unwrap();
        assert!(b.text.contains("[Thinking]"));
        assert!(b.text.contains("reasoning step"));
        assert!(b.text.contains("```python"));

        let v2 = serde_json::json!({
            "_v": 3,
            "type": 2,
            "text": "Here is code:",
            "codeBlocks": [{ "content": "x = 1" }]
        });
        let b2 = decode_l3_bubble_blob("bid-cb", &v2).unwrap();
        assert!(b2.text.contains("Here is code:"));
        assert!(b2.text.contains("x = 1"));
    }

    /// L3 timestamp fallback: timingInfo.clientRpcSendTime when createdAt missing.
    #[test]
    fn decode_l3_bubble_blob_timestamp_fallback() {
        let v = serde_json::json!({
            "_v": 3,
            "type": 1,
            "text": "hi",
            "timingInfo": { "clientRpcSendTime": 1_700_000_000_123_i64 }
        });
        let b = decode_l3_bubble_blob("bid-ts", &v).unwrap();
        assert_eq!(b.created_at_ms, 1_700_000_000_123);
    }

    /// 3-way merge: L3 is the main chain. L2 with the same id overlays
    /// only non-empty fields (LWW). L1 fills gaps.
    #[test]
    fn merge_three_way_l3_wins_over_l2_on_empty_overlay() {
        // L3 has full bubble. L2 has same id but empty text (degraded
        // shape) — must NOT clobber L3.
        let mut l3_bubble = Bubble {
            id: "shared".to_string(),
            role: "user".to_string(),
            text: "rich text".to_string(),
            tool_calls: vec![BubbleToolUse {
                name: "Glob".into(),
                input: None,
            }],
            files: vec!["a.rs".into()],
            created_at_ms: 1000,
            parent_bubble_id: None,
        };
        l3_bubble.tool_calls[0].input = Some(serde_json::json!({"pattern": "*"}));
        let l2_bubble = Bubble {
            id: "shared".to_string(),
            role: "user".to_string(),
            text: String::new(), // empty → must not overwrite
            tool_calls: Vec::new(),
            files: Vec::new(),
            created_at_ms: 0, // 0 → must not overwrite
            parent_bubble_id: None,
        };
        let merged = merge_bubbles_three_way(Vec::new(), vec![l2_bubble], vec![l3_bubble]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "rich text", "L3 text preserved");
        assert_eq!(merged[0].tool_calls.len(), 1, "L3 tool_calls preserved");
        assert_eq!(merged[0].files, vec!["a.rs".to_string()]);
        assert_eq!(merged[0].created_at_ms, 1000, "L3 ts preserved");
    }

    /// 3-way merge: L1 bubbles fill gaps when neither L3 nor L2 have
    /// the bubble id (e.g. a CLI-only session never synced to L2/L3).
    #[test]
    fn merge_three_way_l1_fills_gap() {
        let l1 = vec![Bubble {
            id: "l1-only".to_string(),
            role: "user".to_string(),
            text: "from jsonl".to_string(),
            tool_calls: Vec::new(),
            files: Vec::new(),
            created_at_ms: 500,
            parent_bubble_id: None,
        }];
        let merged = merge_bubbles_three_way(l1, Vec::new(), Vec::new());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "l1-only");
        assert_eq!(merged[0].text, "from jsonl");
    }

    /// 3-way merge: same id across L1 + L3 must dedup to one row.
    #[test]
    fn merge_three_way_dedup_by_id() {
        let l1 = vec![Bubble {
            id: "shared".to_string(),
            role: "user".to_string(),
            text: "from jsonl".to_string(),
            tool_calls: Vec::new(),
            files: Vec::new(),
            created_at_ms: 0,
            parent_bubble_id: None,
        }];
        let l3 = vec![Bubble {
            id: "shared".to_string(),
            role: "user".to_string(),
            text: "from desktop".to_string(),
            tool_calls: Vec::new(),
            files: Vec::new(),
            created_at_ms: 1000,
            parent_bubble_id: None,
        }];
        let merged = merge_bubbles_three_way(l1, Vec::new(), l3);
        assert_eq!(merged.len(), 1, "same id → one row");
        assert_eq!(merged[0].text, "from desktop", "L3 wins as main chain");
    }

    /// 3-way merge: final ordering by `(created_at_ms ASC, id ASC)` —
    /// guarantees stable render order for the React `<MessageList>`.
    #[test]
    fn merge_three_way_preserves_order() {
        let l3 = vec![
            Bubble {
                id: "c".into(),
                role: "user".into(),
                text: "third".into(),
                tool_calls: vec![],
                files: vec![],
                created_at_ms: 3000,
                parent_bubble_id: None,
            },
            Bubble {
                id: "a".into(),
                role: "user".into(),
                text: "first".into(),
                tool_calls: vec![],
                files: vec![],
                created_at_ms: 1000,
                parent_bubble_id: None,
            },
            Bubble {
                id: "b".into(),
                role: "user".into(),
                text: "second".into(),
                tool_calls: vec![],
                files: vec![],
                created_at_ms: 2000,
                parent_bubble_id: None,
            },
        ];
        let merged = merge_bubbles_three_way(Vec::new(), Vec::new(), l3);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].id, "a");
        assert_eq!(merged[1].id, "b");
        assert_eq!(merged[2].id, "c");
    }

    /// `read_conversation` must keep its public signature intact:
    /// returns `Conversation { uuid, bubbles, source_path, total_lines,
    /// parse_errors }`. Frontend's `getConversation(uuid)` in
    /// `src/lib/tauri.ts` types this shape — adding a field would force
    /// TS changes too. `#[serde(default)]` on the new Bubble fields
    /// keeps existing payloads compatible.
    #[test]
    fn read_conversation_returns_unchanged_signature() {
        let conv = read_conversation("definitely-not-a-real-uuid-zzzzz");
        assert_eq!(conv.uuid, "definitely-not-a-real-uuid-zzzzz");
        assert!(conv.bubbles.is_empty());
        assert!(conv.source_path.is_none());
        assert_eq!(conv.total_lines, 0);
        assert_eq!(conv.parse_errors, 0);
    }

    /// Live probe: confirm `reverse_chat_root_via_projects` /
    /// `reverse_chat_root_via_workspace_storage` resolve at least one
    /// real chat_root on the dev machine. This guards the
    /// "no more 30+ orphan chat-<md5> rows" promise from #99/#100.
    /// Skipped by default — run with:
    ///   cargo test canonical::tests::probe_reverse_chat_root_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_reverse_chat_root_real() {
        let chats = match std::fs::read_dir("/home/eric/.cursor/chats") {
            Ok(d) => d,
            Err(_) => return, // dev machine may not have chats/ — silent no-op
        };
        let mut hit_ws = 0usize;
        let mut miss = 0usize;
        for entry in chats.flatten() {
            let chat_root = entry.file_name().to_string_lossy().into_owned();
            if chat_root.len() != 32 { continue; } // skip non-md5 (shouldn't happen but defensive)
            if let Some(slug) = reverse_chat_root_via_workspace_storage(&chat_root) {
                eprintln!("chat_root={chat_root} → workspaceStorage.slug={slug}");
                hit_ws += 1;
            } else {
                eprintln!("chat_root={chat_root} → UNRESOLVED (stays as chat-<md5>)");
                miss += 1;
            }
        }
        // Diagnostic dump: print md5 of every workspaceStorage folder
        // so we can see what cwd → md5 mapping Cursor actually has on
        // this dev box. Useful for understanding why the workspaceStorage
        // reverse-lookup may or may not match the chat_root md5s above.
        if let Ok(ws) = std::fs::read_dir("/home/eric/.config/Cursor/User/workspaceStorage") {
            eprintln!("---workspaceStorage → md5(folder)---");
            for entry in ws.flatten() {
                let hash = entry.file_name().to_string_lossy().into_owned();
                let body = std::fs::read_to_string(entry.path().join("workspace.json")).unwrap_or_default();
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                let folder = v["folder"].as_str().unwrap_or("");
                let path = folder.strip_prefix("file://").unwrap_or(folder);
                let md5 = paths::chat_root_for(path);
                eprintln!("hash={hash} path={path} md5={md5}");
            }
        }
        eprintln!("SUMMARY: workspaceStorage={hit_ws} unresolved={miss}");
        assert!(
            hit_ws > 0,
            "no chat_root resolved via workspaceStorage — reverse-lookup is broken"
        );
    }
}
