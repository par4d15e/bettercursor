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
            let uuid = t_entry.file_name().to_string_lossy().trim_end_matches(".jsonl").to_string();
            let path = t_entry.path();
            let modified = file_mtime_ms(&path).unwrap_or(0);
            let (preview, _files) = read_jsonl_preview(&path);

            merge_source(
                by_uuid,
                &uuid,
                SourceInfo {
                    last_seen_at: modified,
                    layer: "1".into(),
                    path: path.display().to_string(),
                },
                SourceLayer::LinuxCli,
                &project_slug,
                "".to_string(),
                modified,
                preview,
            );
        }
    }
}

fn read_jsonl_preview(path: &Path) -> (String, Vec<String>) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return (String::new(), vec![]);
    };
    let mut preview = String::new();
    let mut files = Vec::new();

    // Take only the first non-empty line for preview (fast).
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(role) = v.get("role").and_then(|r| r.as_str()) {
                if role == "user" && preview.is_empty() {
                    if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
                        preview = content.chars().take(120).collect();
                    } else if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
                        for item in arr {
                            if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                    preview = text.chars().take(120).collect();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if let Some(attachments) = v.get("attachments").and_then(|a| a.as_array()) {
                for a in attachments {
                    if let Some(name) = a.get("name").and_then(|n| n.as_str()) {
                        files.push(name.to_string());
                    }
                }
            }
        }
    }
    (preview, files)
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
