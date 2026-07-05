//! bettercursor fs watcher — auto-sync local Cursor sessions.
//!
//! v0.2 MVP design:
//!   - Watch the 3 parent dirs (Layer 1 projects, Layer 2 chats,
//!     Layer 3 globalStorage) with `notify` (inotify/fsevent).
//!   - Debounce bursts of events into a single re-scan.
//!   - On debounce flush, run a full `scan_all()` and emit
//!     `sessions-updated` to the frontend. No incremental merge —
//!     matching the Python reference's hot-reload semantics.
//!   - Optional polling fallback every 30s (catches anything notify
//!     missed on weird FS / cross-mount); notify handles 95% of cases.
//!
//! v0.2-alpha simplification: there is **no user toggle** anymore.
//! The watcher thread always runs, and `run_scan` always re-scans on
//! fs events. Previously a `set_auto_sync` toggle gated `run_scan`
//! but the listener itself stayed alive regardless — so the toggle
//! didn't actually save any resources and only confused users
//! ("why is the toggle in the way?"). Removed in #103.
//!
//! Future work (out of MVP scope):
//!   - True incremental merge keyed by uuid
//!   - Write-side fs actions (broken-session repair button)
//!   - Per-workspace state.vscdb watchers

use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use super::canonical;
use super::paths;
use crate::AppState;

/// Debounce window — burst window for collapsing multiple fs events
/// from a single save (Layer 1 JSONLs and SQLite WAL flushes can emit
/// 5–20 events in <100ms) into one re-scan.
const DEBOUNCE_MS: u64 = 500;

/// Polling interval — fallback so we still catch things notify dropped
/// (e.g. mtime-only updates inside a SQLite file).
const POLL_INTERVAL_SECS: u64 = 30;

/// Spawn the watcher thread. Returns immediately; the thread lives
/// for the lifetime of the `app` (drops when the process exits).
///
/// The thread ALWAYS starts — even when the user has the auto-sync
/// toggle OFF — because keeping `notify` registered avoids inotify
/// handle churn on every toggle and means enabling the preference
/// later takes effect with zero latency. The actual gate on whether
/// to run `scan_all()` lives inside `run_scan`.
pub fn spawn(app: AppHandle) -> Result<()> {
    // Idempotency: skip if a watcher thread is already running.
    if let Some(state) = app.try_state::<AppState>() {
        let flag = state.watcher_active.lock().unwrap();
        if *flag {
            return Ok(());
        }
    }

    let dirs = resolve_watch_dirs()?;
    if dirs.is_empty() {
        log::warn!("fs watcher: no Cursor dirs found, skipping spawn");
        return Ok(());
    }

    // Mark as active before we move `app` into the closure.
    if let Some(state) = app.try_state::<AppState>() {
        *state.watcher_active.lock().unwrap() = true;
    }

    thread::spawn(move || {
        if let Err(e) = run_watcher(app.clone(), dirs) {
            log::warn!("fs watcher exited: {e:#}");
        }
        if let Some(state) = app.try_state::<AppState>() {
            *state.watcher_active.lock().unwrap() = false;
        }
    });
    Ok(())
}

/// Resolve the directories we should watch. Returns only dirs that
/// exist on disk — we lazily create missing parents (Layer 2 may not
/// exist on a fresh install with no CLI use).
fn resolve_watch_dirs() -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for d in [
        paths::cursor_projects_dir(),
        paths::chats_dir(),
        paths::global_db_path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default(),
        paths::workspace_storage_dir().ok().unwrap_or_default(),
    ] {
        if !d.as_os_str().is_empty() && d.exists() {
            out.push(d);
        }
    }
    Ok(out)
}

/// Watcher thread body. Owns its own `RecommendedWatcher` and the
/// notify→scan pipeline.
fn run_watcher(app: AppHandle, dirs: Vec<PathBuf>) -> Result<()> {
    let (tx, rx) = channel::<notify::Result<Event>>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;
    for d in &dirs {
        if let Err(e) = watcher.watch(d, RecursiveMode::Recursive) {
            log::warn!("watcher: failed to watch {}: {e}", d.display());
        } else {
            log::info!("watcher: watching {}", d.display());
        }
    }
    let _hold_watcher = Arc::new(watcher); // keep alive while we have `rx`

    let last_scan = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(POLL_INTERVAL_SECS)));
    let dirty = Arc::new(Mutex::new(false));

    // fs events → mark dirty, debounce by timestamp
    let app_for_events = app.clone();
    let dirty_events = dirty.clone();
    let last_scan_events = last_scan.clone();
    let events_handle = thread::spawn(move || loop {
        match rx.recv() {
            Ok(Ok(ev)) => {
                if is_interesting(&ev) {
                    *dirty_events.lock().unwrap() = true;
                }
            }
            Ok(Err(e)) => log::warn!("watcher: notify error: {e}"),
            Err(_) => break, // tx dropped → watcher thread exiting
        }
        // debounce: sleep DEBOUNCE_MS, see if dirty, scan if so
        thread::sleep(Duration::from_millis(DEBOUNCE_MS));
        if *dirty_events.lock().unwrap() {
            *dirty_events.lock().unwrap() = false;
            *last_scan_events.lock().unwrap() = Instant::now();
            run_scan(&app_for_events, "fs-event");
        }
    });

    // polling fallback (catches notify-edge-cases)
    loop {
        thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        let last = *last_scan.lock().unwrap();
        if last.elapsed() >= Duration::from_secs(POLL_INTERVAL_SECS) {
            run_scan(&app, "poll");
            *last_scan.lock().unwrap() = Instant::now();
        }
        // if both threads died (shouldn't happen), bail
        if events_handle.is_finished() {
            break;
        }
    }
    Ok(())
}

/// Filter: only care about Create / Modify / Remove / ModifyName events.
/// Access & Other are noise.
fn is_interesting(ev: &Event) -> bool {
    matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Single re-scan + emit. Errors are logged but never abort — a single
/// failed scan must not kill the watcher thread.
///
/// v0.2-alpha: no gate. The watcher thread always runs and always
/// re-scans on fs events; there is no user toggle. This matches the
/// product expectation that "open bettercursor and see current
/// sessions" works out of the box.
fn run_scan(app: &AppHandle, trigger: &str) {
    match canonical::visible_sessions() {
        Ok(sessions) => {
            let count = sessions.len();
            if let Some(state) = app.try_state::<AppState>() {
                *state.sessions.lock().unwrap() = sessions;
                *state.last_scan_at.lock().unwrap() = Some(chrono::Utc::now());
            }
            let _ = app.emit("sessions-updated", count);
            log::debug!("auto-sync [{trigger}]: {count} sessions");
        }
        Err(e) => {
            log::warn!("auto-sync [{trigger}] failed: {e:#}");
        }
    }
}

/// Public API for the frontend: turn the watcher on. Idempotent.
pub fn start(app: &AppHandle) -> Result<()> {
    spawn(app.clone())
}

/// Return the resolved watch dirs (for diagnostics / future status UI).
pub fn watched_dirs() -> Vec<PathBuf> {
    resolve_watch_dirs().unwrap_or_default()
}

/// Stable display name for one watched dir (used by diagnostics).
pub fn dir_label(p: &Path) -> String {
    let s = p.display().to_string();
    if let Some(home) = home::home_dir() {
        let home_s = home.display().to_string();
        if let Some(suffix) = s.strip_prefix(&home_s) {
            return format!("~{suffix}");
        }
    }
    s
}

// Keep the unused-import lint quiet during initial scaffold.
#[allow(dead_code)]
fn _ensure_path_in_scope() {
    let _: &Path = Path::new("");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_strips_home() {
        let home = home::home_dir().unwrap_or_default();
        let p = home.join(".cursor").join("projects");
        let label = dir_label(&p);
        assert!(label.starts_with("~"));
    }

    #[test]
    fn interesting_filters_access_events() {
        let ev = Event {
            kind: EventKind::Access(notify::event::AccessKind::Open(
                notify::event::AccessMode::Any,
            )),
            paths: vec![],
            attrs: Default::default(),
        };
        assert!(!is_interesting(&ev));
    }
}
