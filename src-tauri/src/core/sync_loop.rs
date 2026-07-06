//! 后台 sync loop — outbox flush + 向已信任 peer 推送 v4 snapshot。

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use crate::core::transport::{LanTcpTransport, PushSnapshot, Transport};

static LOOP_STARTED: AtomicBool = AtomicBool::new(false);

/// 启动 5 分钟周期的 outbox flush + trusted peer push（幂等）。
pub fn start_background_sync(app: tauri::AppHandle) {
    if LOOP_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let port = match crate::core::transport::lan::ensure_lan_server_public() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("LAN server not started for background sync: {e:#}");
            0
        }
    };
    let device_id = device_id();
    let device_name = local_hostname();
    if port > 0 {
        match crate::core::discovery::spawn_mdns_service(&device_id, &device_name, port) {
            Ok(()) => {
                log::info!(
                    "mDNS advertise started: device_name={} port={}",
                    device_name,
                    port
                );
            }
            Err(e) => {
                log::warn!("mDNS advertise start failed: {e:#}");
            }
        }
    }

    std::thread::Builder::new()
        .name("bettercursor-sync-loop".into())
        .spawn(move || loop {
            let interval_secs = crate::core::config::load().auto_pull_interval_secs.max(60);
            std::thread::sleep(Duration::from_secs(interval_secs));
            if let Err(e) = tick(&app) {
                log::warn!("background sync tick failed: {e:#}");
            }
        })
        .ok();
}

fn tick(app: &tauri::AppHandle) -> Result<()> {
    flush_all_outboxes()?;
    push_dirty_sessions(app)?;
    let prefs = crate::core::config::load();
    if prefs.auto_pull_enabled {
        pull_from_trusted_peers()?;
    }
    Ok(())
}

fn pull_from_trusted_peers() -> Result<()> {
    let peers = crate::core::transport::trusted_peers::TrustedPeersFile::load()?;
    for peer in &peers.peers {
        match crate::core::transport_pull::pull_and_apply_from_peer(&peer.id, 0) {
            Ok(report) => {
                if report.failed > 0 {
                    log::warn!(
                        "auto-pull from {}: {} failed of {}",
                        peer.id,
                        report.failed,
                        report.count
                    );
                }
            }
            Err(e) => log::warn!("auto-pull from {} failed: {e:#}", peer.id),
        }
    }
    Ok(())
}

fn flush_all_outboxes() -> Result<()> {
    let peers = crate::core::transport::trusted_peers::TrustedPeersFile::load()?;
    for peer in &peers.peers {
        let pending = crate::core::transport::outbox::list_pending(&peer.id)?;
        if pending.is_empty() {
            continue;
        }
        let transport = LanTcpTransport::new(peer.clone());
        let rt = tokio::runtime::Runtime::new()?;
        for path in pending {
            let body = std::fs::read_to_string(&path)?;
            let snap: crate::core::snapshot::SessionSnapshot =
                crate::core::snapshot::decode_snapshot(&body)?;
            let payload = PushSnapshot::V4(snap);
            match rt.block_on(transport.push(&payload)) {
                Ok(_) => {
                    let _ = crate::core::transport::outbox::mark_processed(&peer.id, &path);
                }
                Err(e) => log::warn!("outbox flush to {} failed: {e:#}", peer.id),
            }
        }
    }
    Ok(())
}

fn push_dirty_sessions(app: &tauri::AppHandle) -> Result<()> {
    let peers = crate::core::transport::trusted_peers::TrustedPeersFile::load()?;
    if peers.peers.is_empty() {
        return Ok(());
    }
    let sessions = app
        .try_state::<crate::AppState>()
        .map(|s| s.sessions.lock().unwrap().clone())
        .unwrap_or_default();
    if sessions.is_empty() {
        return Ok(());
    }
    let host = local_hostname();
    let now_ms = chrono::Utc::now().timestamp_millis();
    for session in sessions.iter().take(20) {
        let conv = crate::core::canonical::read_conversation(&session.uuid);
        if conv.bubbles.is_empty() {
            continue;
        }
        let snap = crate::core::snapshot::SessionSnapshot::from_canonical_v4(
            session,
            &conv.bubbles,
            &host,
            now_ms,
        );
        let body = crate::core::snapshot::encode_snapshot(&snap)?;
        for peer in &peers.peers {
            let transport = LanTcpTransport::new(peer.clone());
            let payload = crate::core::transport::PushSnapshot::V4(snap.clone());
            let rt = tokio::runtime::Runtime::new()?;
            if let Err(e) = rt.block_on(transport.push(&payload)) {
                log::warn!(
                    "auto push {} to {} failed, enqueue outbox: {e:#}",
                    session.uuid,
                    peer.id
                );
                let _ = crate::core::transport::outbox::enqueue(&peer.id, &session.uuid, &body);
            }
        }
    }
    Ok(())
}

fn device_id() -> String {
    let path = crate::core::paths::bettercursor_dir().join("device_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let t = existing.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let id = hex::encode(rand_bytes(16));
    let _ = std::fs::create_dir_all(crate::core::paths::bettercursor_dir());
    let _ = std::fs::write(&path, &id);
    id
}

fn rand_bytes(n: usize) -> Vec<u8> {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64;
    let mut out = Vec::with_capacity(n);
    let mut x = seed;
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push((x & 0xFF) as u8);
    }
    out
}

fn local_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
