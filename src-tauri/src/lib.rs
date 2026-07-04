//! bettercursor — Tauri + Rust read-only viewer for local Cursor sessions.
//!
//! See:
//!   - PRD.md             — product requirements
//!   - TAURI_RUST_PLAN.md — technical plan
//!
//! Architecture:
//!   - core::paths    — 4-layer storage path resolution (Mac / Linux)
//!   - core::storage  — WAL-safe SQLite read
//!   - core::canonical— merge sessions across layers (Phase T1)

mod core;

use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

/// Application-wide state, managed by Tauri.
pub struct AppState {
    pub sessions: Mutex<Vec<core::canonical::CanonicalSession>>,
    pub last_scan_at: Mutex<Option<chrono::DateTime<chrono::Utc>>>,
    /// True iff the fs-watcher thread is currently alive. Used so
    /// the spawn site stays idempotent across restart attempts.
    pub watcher_active: Mutex<bool>,
}

impl AppState {
    fn new() -> Self {
        // v0.2-alpha: the watcher thread always runs and always re-scans
        // on fs events. There's no user toggle to gate it anymore —
        // see watcher::run_scan. We don't load any prefs at startup.
        Self {
            sessions: Mutex::new(Vec::new()),
            last_scan_at: Mutex::new(None),
            watcher_active: Mutex::new(false),
        }
    }
}

/// Return all canonical sessions. Empty list on first launch before refresh.
///
/// Tauri command — invoked from the React frontend via `invoke('list_sessions')`.
#[tauri::command]
fn list_sessions(state: State<'_, AppState>) -> Vec<core::canonical::CanonicalSession> {
    state.sessions.lock().unwrap().clone()
}

/// Force a fresh scan of the local Cursor storage layers, replace the cache,
/// and emit `sessions-updated` so the UI refreshes.
///
/// Tauri command — invoked from the React frontend via `invoke('refresh_sessions')`.
#[tauri::command]
fn refresh_sessions(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let sessions = core::canonical::scan_all().map_err(|e| e.to_string())?;
    let count = sessions.len();
    *state.sessions.lock().unwrap() = sessions;
    *state.last_scan_at.lock().unwrap() = Some(chrono::Utc::now());
    let _ = app.emit("sessions-updated", count);
    Ok(count)
}

/// Build the resume command appropriate for the given source.
///
/// Tauri command — invoked from the React frontend via
/// `invoke('get_resume_command', { uuid, source })`.
#[tauri::command]
fn get_resume_command(uuid: &str, source: &str) -> String {
    match source {
        "linux_cli" => format!("cursor-agent --resume {uuid}"),
        // mac / linux_desktop: open Cursor with the resume target
        _ => format!("open -a Cursor --args --resume {uuid}"),
    }
}

/// Get the current platform string for debugging.
#[tauri::command]
fn platform_info() -> String {
    format!(
        "{} ({})",
        std::env::consts::OS,
        core::paths::cursor_user_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<not found>".to_string())
    )
}

/// Load the full conversation (bubbles + tool calls + attachments) for a
/// single session uuid from Layer 1 JSONL.
///
/// Tauri command — invoked from the React frontend via
/// `invoke('get_conversation', { uuid })`. Returns a `Conversation`
/// with `source_path = None` if no Layer 1 JSONL was found.
#[tauri::command]
fn get_conversation(uuid: &str) -> core::canonical::Conversation {
    core::canonical::read_conversation(uuid)
}

/// Watcher diagnostics for the frontend. Returns the watch dirs
/// (with `~` substituted for `$HOME`) and whether the watcher thread
/// is currently alive. The "enabled" flag was removed in v0.2-alpha:
/// the watcher always runs and always re-scans on fs events.
#[derive(serde::Serialize)]
struct WatcherStatus {
    active: bool,
    dirs: Vec<String>,
}

#[tauri::command]
fn watcher_status(state: State<'_, AppState>) -> WatcherStatus {
    WatcherStatus {
        active: *state.watcher_active.lock().unwrap(),
        dirs: core::watcher::watched_dirs()
            .iter()
            .map(|p| core::watcher::dir_label(p))
            .collect(),
    }
}

/// One-click L2↔L3 sync for a single session. The frontend invokes
/// this from the SessionDetail "补层同步" button (see v0.2-alpha plan).
///
/// `cwd` is supplied by the frontend because the CanonicalSession
/// doesn't currently carry it (we expose `project_path` which is
/// sourced from L3's `workspaceIdentifier.uri.fsPath`; if that's
/// missing, we try `chat_root_for` reverse-lookup by scanning
/// `~/.cursor/chats/*/`. If both fail, sync returns
/// `skipped=["no_cwd"]`.)
#[tauri::command]
async fn sync_session_layer23(
    uuid: String,
    cwd: Option<String>,
) -> Result<core::sync::SyncReport, String> {
    let resolved_cwd = match cwd {
        Some(c) if !c.trim().is_empty() => c,
        _ => lookup_cwd_for_session(&uuid).unwrap_or_default(),
    };
    core::sync::sync_session(&uuid, &resolved_cwd).map_err(|e| e.to_string())
}

/// v0.2.1: 全量扫所有 chats/<md5>/<uuid>/store.db, 把每条
/// `meta[0].latestRootBlobId` 是空字符串的 session 修上. 修之前
/// 自动备份 store.db 到 `<store.db>.backup_<ts>`. 由前端手动触发
/// (SessionTree 头部 Wrench 按钮 / SessionDetail 单条"修复"按钮).
///
/// Returns: 修了多少 (`fixed`)、跳过了多少 (`scipped`)、扫过多少
/// (`scanned`).
#[tauri::command]
fn fix_orphans() -> Result<core::sync::FixOrphansReport, String> {
    core::sync::fix_orphans().map_err(|e| e.to_string())
}

/// v0.2.1: 删除一条 session 的 Layer 1 (JSONL) + Layer 2 (store.db)
/// 目录. Layer 3 (state.vscdb composerData) 强制跳过 (Cursor Desktop
/// 自己管, 强制写可能损坏 workspace storage).
///
/// 前置 `cursor_processes_running` 守卫 — 跟 sync_session_layer23 一致.
/// `project_slug` 来自 CanonicalSession 的 project_slug 字段, 后端不重算
/// (避免 L1 路径猜错). 当 slug 为 None 时跳过 L1 (只删 L2).
#[tauri::command]
async fn delete_session(
    uuid: String,
    cwd: Option<String>,
    project_slug: Option<String>,
) -> Result<core::sync::DeleteReport, String> {
    let cwd_str = cwd.unwrap_or_default();
    core::sync::delete_session(&uuid, &cwd_str, project_slug.as_deref())
        .map_err(|e| e.to_string())
}

/// Reverse-lookup: scan `~/.cursor/chats/*/<uuid>/` for a matching
/// directory and return the L2 directory basename (which is
/// `md5(cwd)`). Used when neither L3 nor L1 surfaced a real cwd.
///
/// We can't reverse MD5 → path, but if the uuid lives under exactly
/// one chats dir we know it's *that* project. For the actual sync
/// to work, however, we still need cwd to compute the L2 dir
/// ourselves — so this helper returns the md5 *basename*, and the
/// caller treats it as a fallback `cwd` (it'll be wrong, but
/// `write_layer2` will write into a different dir than where the
/// existing session lives; that's a sync failure we surface).
///
/// In practice the frontend always sends `cwd` from the session's
/// `project_path`, so this fallback is only hit in edge cases.
fn lookup_cwd_for_session(uuid: &str) -> Option<String> {
    let chats = core::paths::chats_dir();
    if !chats.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&chats).ok()?;
    let mut hits = Vec::new();
    for entry in entries.flatten() {
        if entry.path().join(uuid).is_dir() {
            hits.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    hits.into_iter().next()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::new())
        .setup(|app| {
            // Initial scan on startup; failures are logged but not fatal —
            // the user can hit Refresh later.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                match core::canonical::scan_all() {
                    Ok(sessions) => {
                        log::info!("initial scan: {} session(s)", sessions.len());
                        if let Some(state) = handle.try_state::<AppState>() {
                            *state.sessions.lock().unwrap() = sessions;
                            *state.last_scan_at.lock().unwrap() = Some(chrono::Utc::now());
                            let _ = handle.emit("sessions-updated", state.sessions.lock().unwrap().len());
                        }
                    }
                    Err(e) => {
                        log::warn!("initial scan failed: {e:#}");
                    }
                }
            });
            // Start the fs watcher for live auto-sync.
            if let Err(e) = core::watcher::start(&app.handle()) {
                log::warn!("fs watcher failed to start: {e:#}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            refresh_sessions,
            get_resume_command,
            platform_info,
            get_conversation,
            watcher_status,
            sync_session_layer23,
            fix_orphans,
            delete_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
