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
    /// User preference: whether the watcher should *fire scans* when
    /// fs events arrive. The watcher thread itself stays alive as
    /// long as the app runs — it just gates the `scan_all()` calls.
    /// Default = loaded from `~/.bettercursor/config.json`, falls
    /// back to `false` (ccswitch-style user-opt-in).
    pub auto_sync_enabled: Mutex<bool>,
}

impl AppState {
    fn new() -> Self {
        // Load user preference from disk. `Preferences::default()`
        // (auto_sync_enabled = false) is the right starting state when
        // no config file exists — matches ccswitch's local-route default.
        let prefs = core::config::load();
        Self {
            sessions: Mutex::new(Vec::new()),
            last_scan_at: Mutex::new(None),
            watcher_active: Mutex::new(false),
            auto_sync_enabled: Mutex::new(prefs.auto_sync_enabled),
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
/// (with `~` substituted for `$HOME`), whether the watcher thread
/// is currently alive, and whether the user has opted in to
/// auto-sync (the ccswitch-style toggle).
#[derive(serde::Serialize)]
struct WatcherStatus {
    active: bool,
    /// True iff the user has enabled the auto-sync toggle. The
    /// watcher thread stays alive regardless, but skips `scan_all()`
    /// calls when this is `false`.
    enabled: bool,
    dirs: Vec<String>,
}

#[tauri::command]
fn watcher_status(state: State<'_, AppState>) -> WatcherStatus {
    WatcherStatus {
        active: *state.watcher_active.lock().unwrap(),
        enabled: *state.auto_sync_enabled.lock().unwrap(),
        dirs: core::watcher::watched_dirs()
            .iter()
            .map(|p| core::watcher::dir_label(p))
            .collect(),
    }
}

/// Toggle the auto-sync preference. Persists to
/// `~/.bettercursor/config.json` so the choice survives restarts.
/// Returns the new full `WatcherStatus` so the frontend can refresh
/// its badge in one IPC round-trip.
#[tauri::command]
fn set_auto_sync(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<WatcherStatus, String> {
    // Persist first — if disk write fails, surface to user immediately
    // rather than silently letting in-memory and disk diverge.
    let prefs = core::config::set_auto_sync(enabled).map_err(|e| e.to_string())?;
    *state.auto_sync_enabled.lock().unwrap() = prefs.auto_sync_enabled;
    log::info!(
        "auto-sync preference toggled: enabled={}",
        prefs.auto_sync_enabled
    );
    Ok(WatcherStatus {
        active: *state.watcher_active.lock().unwrap(),
        enabled: prefs.auto_sync_enabled,
        dirs: core::watcher::watched_dirs()
            .iter()
            .map(|p| core::watcher::dir_label(p))
            .collect(),
    })
}

/// Inject a CLI-originated session into Cursor Desktop's Layer 3
/// (state.vscdb) so the Electron Sidebar shows it. **Offline only**:
/// bettercursor never writes to `state.vscdb` while Cursor Electron
/// holds it open — the rename race silently lost mutations (#84).
/// Instead, three Tauri commands front the offline pipeline:
///   1) `dry_run_inject_layer3(uuid)` — pure read; returns an
///      `InjectPlan` listing every SQLite upsert that *would* run
///      so the UI can preview the diff.
///   2) `prepare_inject_layer3(uuid)` — writes a JSON envelope of
///      that plan to `~/.bettercursor/queue/inject-<uuid>.json`
///      and returns the queue path + the `python3 ...` apply
///      command for the UI to show.
///   3) `inspect_prepared_layer3(uuid)` — checks the queue file +
///      whether `apply.py` has already run (by sidecar
///      `.applied` marker). Used by the UI badge for "已应用".
///
/// The user must (a) quit Cursor Electron and (b) run the apply
/// command themselves. After it succeeds they reopen Cursor; the
/// Sidebar will then show the new entry.
#[tauri::command]
fn dry_run_inject_layer3(uuid: &str) -> Result<core::inject::InjectPlan, String> {
    core::inject::dry_run_inject(uuid).map_err(|e| e.to_string())
}

#[tauri::command]
fn prepare_inject_layer3(uuid: &str) -> Result<core::inject::PrepareResult, String> {
    // First use: copy the bundled apply.py into place so the user's
    // apply_command actually resolves. Best-effort; failure is
    // surfaced verbatim so we never queue work the user can't apply.
    core::inject::ensure_apply_script().map_err(|e| e.to_string())?;
    core::inject::prepare_inject(uuid).map_err(|e| e.to_string())
}

#[tauri::command]
fn inspect_prepared_layer3(uuid: &str) -> Option<core::inject::Prepared> {
    core::inject::inspect_prepared(uuid)
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
            set_auto_sync,
            dry_run_inject_layer3,
            prepare_inject_layer3,
            inspect_prepared_layer3,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
