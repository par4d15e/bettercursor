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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bubble {
    pub role: String,
    pub text: String,
    pub tool_calls: Vec<BubbleToolUse>,
    pub files: Vec<String>,
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
    pub sources: Sources,
    pub first_user_message_preview: String,
    pub files_referenced: Vec<String>,
    /// Concatenated text from the conversation (first ~2 KB), used for
    /// full-content search on the frontend. Populated from Layer 1.
    #[serde(default)]
    pub indexable_text: String,
}

// ── Entry point ───────────────────────────────────────────────

/// Scan all 3 storage layers on the current host and return merged sessions.
pub fn scan_all() -> Result<Vec<CanonicalSession>> {
    let mut by_uuid: HashMap<String, CanonicalSession> = HashMap::new();

    scan_layer1_into(&mut by_uuid);
    scan_layer2_into(&mut by_uuid);
    scan_layer3_into(&mut by_uuid);

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

            merge_source(
                by_uuid,
                &uuid,
                SourceInfo {
                    last_seen_at: modified,
                    layer: "1".into(),
                    path: display_path,
                },
                SourceLayer::LinuxCli,
                &project_slug,
                "".to_string(),
                modified,
                preview,
            );

            // `indexable_text` is set directly (not via merge_source) because
            // it's a per-Layer-1-only property; first writer wins.
            if let Some(entry) = by_uuid.get_mut(&uuid) {
                if entry.indexable_text.is_empty() {
                    entry.indexable_text = indexable;
                }
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

            let (name, created_at, blob_count, _has_root) = read_store_db_meta(&store_db);
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

            // Update bubble_count and last_updated_at on the merged entry.
            if let Some(entry) = by_uuid.get_mut(&uuid) {
                entry.bubble_count = entry.bubble_count.max(blob_count);
                if modified > entry.last_updated_at {
                    entry.last_updated_at = modified;
                }
                if entry.chat_root.is_empty() {
                    entry.chat_root = chat_root.clone();
                }
            }
        }
    }
}

fn read_store_db_meta(path: &Path) -> (String, i64, u32, bool) {
    let Ok(r) = storage::open_read(path) else {
        return (String::new(), 0, 0, false);
    };

    // Read meta[0] (agent session metadata).
    let (name, created_at, has_root) = match r.get_json("0", "meta") {
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
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            (name, created_at, root)
        }
        _ => (String::new(), 0, false),
    };

    // Count blobs.
    let blob_count = r.list_blob_ids().map(|v| v.len() as u32).unwrap_or(0);

    (name, created_at, blob_count, has_root)
}

fn derive_slug_from_chat_root(chat_root: &str) -> String {
    // Best effort: scan ~/.cursor/projects/* for a directory whose
    // 0-byte (placeholder) content matches. Otherwise return chat_root.
    // v0.1 just uses the chat_root as a synthetic slug.
    format!("chat-{chat_root}")
}

// ── Layer 3: Electron state.vscdb ─────────────────────────────

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
                let project_path = v
                    .get("workspaceIdentifier")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let project_slug = if !project_path.is_empty() {
                    paths::sanitize_project_path(&project_path)
                } else {
                    format!("desktop-{uuid}")
                };
                let created_at = v
                    .get("createdAt")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
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
                    modified.max(created_at),
                    String::new(),
                );
                if let Some(entry) = by_uuid.get_mut(uuid) {
                    entry.bubble_count = entry.bubble_count.max(bubble_count);
                    if entry.project_path.is_empty() && !project_path.is_empty() {
                        entry.project_path = project_path;
                    }
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
        let workspace = c
            .get("workspaceIdentifier")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let project_slug = if !workspace.is_empty() {
            paths::sanitize_project_path(&workspace)
        } else {
            format!("desktop-{uuid}")
        };
        let created_at = c.get("createdAt").and_then(|x| x.as_i64()).unwrap_or(0);
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
            modified.max(created_at),
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
        sources: Sources::default(),
        first_user_message_preview: first_user_message_preview.clone(),
        files_referenced: vec![],
        indexable_text: String::new(),
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
    entry.is_empty_draft = entry.bubble_count == 0
        && entry.sources.mac.is_none()
        && entry.sources.linux_cli.is_none()
        && entry.sources.linux_desktop.is_none();
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

/// Locate the Layer 1 JSONL transcript for a given session uuid.
///
/// Cursor stores each session as either:
///   (a) `<chat_root>/agent-transcripts/<uuid>/<uuid>.jsonl`, or
///   (b) `<chat_root>/agent-transcripts/<uuid>.jsonl` (older layout).
///
/// Either is returned when present.
pub fn find_jsonl_for(uuid: &str) -> Option<PathBuf> {
    let projects = paths::cursor_projects_dir();
    if !projects.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&projects).ok()?;
    for project in entries.flatten() {
        let transcripts = project.path().join("agent-transcripts");
        if !transcripts.is_dir() {
            continue;
        }
        let in_dir = transcripts.join(uuid).join(format!("{uuid}.jsonl"));
        if in_dir.is_file() {
            return Some(in_dir);
        }
        let flat = transcripts.join(format!("{uuid}.jsonl"));
        if flat.is_file() {
            return Some(flat);
        }
    }
    None
}

/// Read all bubbles from the Layer 1 JSONL for `uuid`.
///
/// Returns an empty `Conversation` (with `source_path = None`) when no
/// JSONL exists. Lines that fail to parse are counted but never panic.
pub fn read_conversation(uuid: &str) -> Conversation {
    match find_jsonl_for(uuid) {
        None => Conversation {
            uuid: uuid.to_string(),
            bubbles: vec![],
            source_path: None,
            total_lines: 0,
            parse_errors: 0,
        },
        Some(path) => read_conversation_from_path(uuid, &path),
    }
}

/// Read all bubbles from an already-resolved JSONL file path.
///
/// Useful for tests and for callers that already know the file location.
pub fn read_conversation_from_path(uuid: &str, path: &Path) -> Conversation {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Conversation {
            uuid: uuid.to_string(),
            bubbles: vec![],
            source_path: Some(path.display().to_string()),
            total_lines: 0,
            parse_errors: 0,
        };
    };

    let mut bubbles: Vec<Bubble> = vec![];
    let mut total_lines = 0usize;
    let mut parse_errors = 0usize;

    for line in body.lines() {
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

        bubbles.push(Bubble {
            role,
            text: text_parts.join("\n\n"),
            tool_calls,
            files,
        });
    }

    Conversation {
        uuid: uuid.to_string(),
        bubbles,
        source_path: Some(path.display().to_string()),
        total_lines,
        parse_errors,
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
}
