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
/// is currently active.
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
