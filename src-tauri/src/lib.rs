//! bettercursor — Tauri + Rust read-only viewer for local Cursor sessions.
//!
//! See:
//!   - PRD.md             — product requirements
//!   - SYNC_DESIGN.md     — cross-device sync design (Transport trait, codecs)
//!   - TAURI_RUST_PLAN.md — technical plan
//!
//! Architecture:
//!   - core::paths    — 4-layer storage path resolution (Mac / Linux)
//!   - core::storage  — WAL-safe SQLite read
//!   - core::canonical— merge sessions across layers (Phase T1)
//!   - core::transport— v0.2.6 cross-device sync (Transport trait + SSH/rsync)

mod core;

use std::sync::Mutex;
use std::time::Instant;
use tauri::{Emitter, Manager, State};

use crate::core::transport::Transport;

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
/// v0.2.3 rename: was `refresh_sessions` (v0.1 terminology). Now `sync_now`
/// matches PRD / SYNC_DESIGN v0.2+ wording. Same semantics — full
/// `canonical::scan_all()` + emit `sessions-updated` + bump `last_scan_at`.
///
/// Tauri command — invoked from the React frontend via `invoke('sync_now')`.
#[tauri::command]
fn sync_now(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let sessions = core::canonical::visible_sessions().map_err(|e| e.to_string())?;
    let count = sessions.len();
    // v0.3.0: mirror into unified.db so the FTS5 mirror,
    // content_hash, and session rows reflect the post-scan world.
    // Best-effort — failures MUST NOT fail the in-memory cache refresh
    // that the frontend depends on.
    if let Ok(unified) = core::unified::UnifiedDb::open() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let host = local_hostname();
        let _ = unified.rebuild_from_cursor_state(&sessions, &host, now_ms);
    }
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
/// is currently alive, and when the last scan completed (epoch ms).
///
/// v0.2.3: `last_scan_at_ms` added so the frontend can render a
/// "12s 前" / "3m 前" counter without re-running a Tauri command
/// every tick. The watcher always runs and always re-scans on fs
/// events (no user toggle — v0.2-alpha #103).
#[derive(serde::Serialize)]
struct WatcherStatus {
    active: bool,
    dirs: Vec<String>,
    /// Epoch ms of the last successful scan (fs event or polling
    /// fallback). `None` before the first scan completes. Frontend
    /// renders this as "Xs 前" / "Xm 前" / "Xh 前".
    last_scan_at_ms: Option<i64>,
}

#[tauri::command]
fn watcher_status(state: State<'_, AppState>) -> WatcherStatus {
    compute_watcher_status(&state)
}

/// Pure helper extracted from `watcher_status` so the unit test can
/// exercise it without spinning up a Tauri runtime. See `tests`
/// module at the bottom of this file.
fn compute_watcher_status(state: &AppState) -> WatcherStatus {
    let last_scan_at_ms = state
        .last_scan_at
        .lock()
        .unwrap()
        .map(|dt| dt.timestamp_millis());
    WatcherStatus {
        active: *state.watcher_active.lock().unwrap(),
        dirs: core::watcher::watched_dirs()
            .iter()
            .map(|p| core::watcher::dir_label(p))
            .collect(),
        last_scan_at_ms,
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
    remove_l3: Option<bool>,
) -> Result<core::sync::DeleteReport, String> {
    let cwd_str = cwd.unwrap_or_default();
    core::sync::delete_session(
        &uuid,
        &cwd_str,
        project_slug.as_deref(),
        remove_l3.unwrap_or(false),
    )
    .map_err(|e| e.to_string())
}

/// v0.2.6: 列出 `~/.bettercursor/transports.json` 里的所有 peer.
#[tauri::command]
fn transport_list_peers() -> Result<Vec<core::transport::PeerSummary>, String> {
    let cfg = core::transport::TransportConfigFile::load().map_err(|e| e.to_string())?;
    Ok(cfg
        .peers
        .into_iter()
        .map(core::transport::PeerSummary::from)
        .collect())
}

/// v0.2.6: 测一个 peer 的 SSH 连通性. 用 `ssh -o BatchMode=yes echo OK`
/// (mock 路径下等价于 fake-ssh.sh). 返回 latency_ms + 可选 error.
///
/// Tauri command — invoked from the React frontend via
/// `invoke('transport_test', { peerId })`.
#[derive(serde::Serialize)]
struct TestReport {
    peer_id: String,
    ok: bool,
    latency_ms: u64,
    error: Option<String>,
}

#[tauri::command]
fn transport_test(peer_id: String) -> Result<TestReport, String> {
    let started = Instant::now();
    let cfg = core::transport::TransportConfigFile::load().map_err(|e| e.to_string())?;
    let peer = cfg
        .peer(&peer_id)
        .ok_or_else(|| format!("peer '{peer_id}' not found in ~/.bettercursor/transports.json"))?
        .clone();
    let transport = core::transport::SshRsyncTransport::new(peer.clone());
    match transport.test_connection() {
        Ok(()) => Ok(TestReport {
            peer_id,
            ok: true,
            latency_ms: started.elapsed().as_millis() as u64,
            error: None,
        }),
        Err(e) => Ok(TestReport {
            peer_id,
            ok: false,
            latency_ms: started.elapsed().as_millis() as u64,
            error: Some(format!("{e:#}")),
        }),
    }
}

/// v0.3.0: 推一条 session 到指定 peer. 从 `AppState.sessions` 找 uuid 对应的
/// `CanonicalSession`, 转 SessionSnapshotMeta, 调 `Transport::push`.
///
/// Tauri command — invoked from the React frontend via
/// `invoke('transport_push', { uuid, peerId })`.
#[tauri::command]
fn transport_push(
    state: State<'_, AppState>,
    uuid: String,
    peer_id: String,
) -> Result<core::transport::PushReport, String> {
    let session = state
        .sessions
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.uuid == uuid)
        .cloned()
        .ok_or_else(|| format!("session '{uuid}' not found in current scan"))?;
    let resolved = core::transport::resolve_peer(&peer_id).map_err(|e| e.to_string())?;
    let conv = core::canonical::read_conversation(&uuid);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let snap = core::snapshot::SessionSnapshot::from_canonical_v4(
        &session,
        &conv.bubbles,
        &local_hostname(),
        now_ms,
    );
    let payload = core::transport::PushSnapshot::V4(snap);
    match resolved {
        core::transport::ResolvedPeer::Ssh(peer) => {
            let transport = core::transport::SshRsyncTransport::new(peer);
            tauri::async_runtime::block_on(transport.push(&payload)).map_err(|e| e.to_string())
        }
        core::transport::ResolvedPeer::Lan(peer) => {
            let transport = core::transport::LanTcpTransport::new(peer);
            tauri::async_runtime::block_on(transport.push(&payload)).map_err(|e| e.to_string())
        }
    }
}

/// v0.3.0: 从指定 peer 拉 snapshot, 经 conflict 分类后写入 unified.db.
/// `since_ms` 默认 0 (拉全部). 仍返回远端元数据列表供 UI 展示.
///
/// Tauri command — invoked from the React frontend via
/// `invoke('transport_pull', { peerId, sinceMs })`.
#[derive(serde::Serialize)]
struct PullReport {
    peer_id: String,
    count: usize,
    snapshots: Vec<core::transport::RemoteSessionMeta>,
}

#[tauri::command]
fn transport_pull(
    peer_id: String,
    since_ms: Option<i64>,
) -> Result<PullReport, String> {
    use core::conflict::{self, ConflictClass};
    use core::transport::{RemoteSnapshot, Transport};

    let resolved = core::transport::resolve_peer(&peer_id).map_err(|e| e.to_string())?;
    let since = since_ms.unwrap_or(0);
    let snaps = match &resolved {
        core::transport::ResolvedPeer::Ssh(peer) => {
            let transport = core::transport::SshRsyncTransport::new(peer.clone());
            tauri::async_runtime::block_on(transport.pull(since))
                .map_err(|e| e.to_string())?
        }
        core::transport::ResolvedPeer::Lan(peer) => {
            let transport = core::transport::LanTcpTransport::new(peer.clone());
            tauri::async_runtime::block_on(transport.pull(since))
                .map_err(|e| e.to_string())?
        }
    };

    let unified = core::unified::UnifiedDb::open().map_err(|e| e.to_string())?;
    let started_ms = chrono::Utc::now().timestamp_millis();
    let run_id = unified
        .record_sync_run(&peer_id, "pull", started_ms)
        .map_err(|e| e.to_string())?;

    let mut processed = 0u32;
    let mut failed = 0u32;
    let mut pull_error: Option<String> = None;

    for snap in &snaps {
        let RemoteSnapshot::V4(v4) = snap else {
            continue;
        };
        let uuid = v4.composer.composer_id.clone();
        let incoming_hash = v4.content_hash();
        let incoming_updated = v4.composer.last_updated_at;
        let local_meta = unified.get_session_meta(&uuid).map_err(|e| e.to_string())?;
        let local_hash = local_meta.as_ref().map(|m| m.content_hash.as_str());
        let local_updated = local_meta
            .as_ref()
            .map(|m| m.last_updated_at_ms)
            .unwrap_or(0);

        let class = conflict::classify(
            local_hash,
            local_updated,
            &incoming_hash,
            incoming_updated,
        );

        let apply_result = (|| -> anyhow::Result<()> {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let project_path = v4.composer.project_path.clone();
            let mut bubbles_to_apply: Option<Vec<core::canonical::Bubble>> = None;

            match class {
                ConflictClass::LocalAhead => return Ok(()),
                ConflictClass::New | ConflictClass::IncomingNewer => {
                    if local_meta.is_some() {
                        let rows = unified.get_bubbles(&uuid)?;
                        let local_bubbles = core::unified::UnifiedDb::bubbles_from_rows(&rows);
                        let payload = conflict::auto_archive_before_overwrite(&local_bubbles);
                        unified.record_archive(&uuid, "before_overwrite", &payload, now_ms)?;
                    }
                    let incoming: Vec<core::canonical::Bubble> =
                        v4.bubbles.iter().map(core::canonical::Bubble::from_snapshot).collect();
                    unified.upsert_session_from_snapshot(v4, &incoming, now_ms)?;
                    bubbles_to_apply = Some(incoming);
                }
                ConflictClass::Identical => {
                    let incoming: Vec<core::canonical::Bubble> =
                        v4.bubbles.iter().map(core::canonical::Bubble::from_snapshot).collect();
                    unified.upsert_session_from_snapshot(v4, &incoming, now_ms)?;
                }
                ConflictClass::Diverged => {
                    let local_bubbles = if local_meta.is_some() {
                        let rows = unified.get_bubbles(&uuid)?;
                        core::unified::UnifiedDb::bubbles_from_rows(&rows)
                    } else {
                        Vec::new()
                    };
                    let incoming: Vec<core::canonical::Bubble> =
                        v4.bubbles.iter().map(core::canonical::Bubble::from_snapshot).collect();
                    let (merged, archive_payload) =
                        conflict::auto_merge(&local_bubbles, &incoming);
                    unified.record_archive(&uuid, "before_auto_merge", &archive_payload, now_ms)?;
                    unified.record_conflict(
                        &uuid,
                        ConflictClass::Diverged,
                        local_hash,
                        Some(&incoming_hash),
                        now_ms,
                    )?;
                    unified.upsert_session_from_snapshot(v4, &merged, now_ms)?;
                    bubbles_to_apply = Some(merged);
                }
            }

            if let Some(bubbles) = bubbles_to_apply {
                let apply = core::sync::apply_session_from_snapshot(
                    &uuid,
                    &project_path,
                    &bubbles,
                )?;
                if !apply.skipped.is_empty() {
                    log::warn!(
                        "transport_pull L2/L3 apply for {uuid} skipped: {:?}",
                        apply.skipped
                    );
                }
            }
            Ok(())
        })();

        match apply_result {
            Ok(()) => processed += 1,
            Err(e) => {
                failed += 1;
                log::warn!("transport_pull apply failed for {uuid}: {e:#}");
                if pull_error.is_none() {
                    pull_error = Some(format!("{uuid}: {e:#}"));
                }
            }
        }
    }

    let finished_ms = chrono::Utc::now().timestamp_millis();
    unified
        .finish_sync_run(
            run_id,
            processed,
            failed,
            finished_ms,
            pull_error.as_deref(),
        )
        .map_err(|e| e.to_string())?;

    let snapshots: Vec<core::transport::RemoteSessionMeta> = snaps
        .iter()
        .map(|s| match s {
            RemoteSnapshot::V4(v) => core::transport::RemoteSessionMeta {
                uuid: v.composer.composer_id.clone(),
                host: v.source_endpoint.host.clone(),
                last_updated_at_ms: v.composer.last_updated_at,
                project_slug: v.composer.project_slug.clone(),
                source_path: v.composer.project_path.clone(),
            },
            RemoteSnapshot::Meta(m) => core::transport::RemoteSessionMeta {
                uuid: m.uuid.clone(),
                host: m.host.clone(),
                last_updated_at_ms: m.last_updated_at_ms,
                project_slug: m.project_slug.clone(),
                source_path: m.source_path.clone(),
            },
        })
        .collect();
    let count = snapshots.len();
    Ok(PullReport {
        peer_id,
        count,
        snapshots,
    })
}

// ── LAN / 配对 / 冲突 Tauri 命令 (v0.3.1) ─────────────────────

#[derive(serde::Serialize)]
struct PairingStartReport {
    code: String,
    port: u16,
}

#[tauri::command]
fn pairing_start() -> Result<PairingStartReport, String> {
    let pending = core::transport::lan::start_pairing_mode().map_err(|e| e.to_string())?;
    let port = core::transport::lan::lan_listen_port().unwrap_or(0);
    Ok(PairingStartReport {
        code: pending.code,
        port,
    })
}

#[tauri::command]
fn pairing_join(
    host: String,
    port: u16,
    code: String,
    device_name: String,
) -> Result<core::transport::trusted_peers::TrustedPeer, String> {
    core::transport::lan::pairing_join_client(&host, port, &code, &device_name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_trusted_peers() -> Result<Vec<core::transport::trusted_peers::TrustedPeer>, String> {
    core::transport::trusted_peers::TrustedPeersFile::load()
        .map(|f| f.peers)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn discovery_browse() -> Result<Vec<core::discovery::DiscoveredDevice>, String> {
    core::discovery::browse_devices(3000).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_unresolved_conflicts() -> Result<Vec<core::unified::ConflictRow>, String> {
    let db = core::unified::UnifiedDb::open().map_err(|e| e.to_string())?;
    db.unresolved_conflicts().map_err(|e| e.to_string())
}

#[tauri::command]
fn resolve_conflict(conflict_id: i64, how: String) -> Result<(), String> {
    let db = core::unified::UnifiedDb::open().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp_millis();
    db.resolve_conflict(conflict_id, &how, now)
        .map_err(|e| e.to_string())
}

/// 本机 hostname. 给 `transport_push` 用, 把本机 hostname 写进 snapshot.
/// 失败 fallback 到 `"unknown"` (push 仍能跑, 但日志会 warn).
fn local_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
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
                match core::canonical::visible_sessions() {
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
            core::sync_loop::start_background_sync(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            sync_now,
            get_resume_command,
            platform_info,
            get_conversation,
            watcher_status,
            sync_session_layer23,
            fix_orphans,
            delete_session,
            transport_list_peers,
            transport_test,
            transport_push,
            transport_pull,
            pairing_start,
            pairing_join,
            list_trusted_peers,
            discovery_browse,
            list_unresolved_conflicts,
            resolve_conflict,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── v0.2.3 unit tests ───────────────────────────────────────────
//
// `compute_watcher_status` is the pure helper extracted from the
// `watcher_status` Tauri command. These tests cover the new
// `last_scan_at_ms` field added in v0.2.3 without needing a Tauri
// runtime.

#[cfg(test)]
mod tests {
    use super::*;

    /// After at least one scan has completed (fs event or polling
    /// fallback), `compute_watcher_status` must surface the timestamp
    /// as epoch ms so the frontend can render "Xs 前".
    #[test]
    fn watcher_status_returns_last_scan_at_ms_after_scan() {
        let state = AppState::new();
        let before = chrono::Utc::now().timestamp_millis();
        *state.last_scan_at.lock().unwrap() = Some(chrono::Utc::now());
        let status = compute_watcher_status(&state);
        let ms = status
            .last_scan_at_ms
            .expect("last_scan_at_ms must be Some after a scan");
        // Same second granularity is fine for "Xs 前" UI; allow 1s slack.
        assert!(
            ms >= before && ms <= before + 1000,
            "expected last_scan_at_ms in [{before}, {}+1000], got {ms}",
            before = before,
            ms = ms,
        );
    }

    /// Before any scan has completed (process just started), the
    /// frontend must see `None` so it can show "尚未扫描" instead of
    /// "1970-01-01 0s 前".
    #[test]
    fn watcher_status_returns_none_before_first_scan() {
        let state = AppState::new();
        assert!(state.last_scan_at.lock().unwrap().is_none());
        let status = compute_watcher_status(&state);
        assert!(
            status.last_scan_at_ms.is_none(),
            "expected last_scan_at_ms None before first scan, got {:?}",
            status.last_scan_at_ms,
        );
    }
}
