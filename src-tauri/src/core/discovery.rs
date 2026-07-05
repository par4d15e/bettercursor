//! mDNS 局域网发现 — `_bettercursor._tcp` 服务广播与浏览。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SERVICE_TYPE: &str = "_bettercursor._tcp.local.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub host: String,
    pub port: u16,
}

/// 本机广播 bettercursor LAN 服务（阻塞式启动守护线程）。
pub fn spawn_mdns_service(device_id: &str, device_name: &str, port: u16) -> Result<()> {
    let device_id = device_id.to_string();
    let device_name = device_name.to_string();
    std::thread::Builder::new()
        .name("bettercursor-mdns".into())
        .spawn(move || {
            if let Err(e) = run_mdns_daemon(&device_id, &device_name, port) {
                log::warn!("mDNS daemon exited: {e:#}");
            }
        })
        .context("spawn mdns thread")?;
    Ok(())
}

fn run_mdns_daemon(device_id: &str, device_name: &str, port: u16) -> Result<()> {
    let daemon = mdns_sd::ServiceDaemon::new().context("mdns ServiceDaemon")?;
    let host = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bettercursor".to_string());
    let instance = format!("{host}.{SERVICE_TYPE}");
    let mut props = HashMap::new();
    props.insert("device_id".into(), device_id.to_string());
    props.insert("device_name".into(), device_name.to_string());
    let info = mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &host,
        &format!("{host}.local."),
        "",
        port,
        Some(props),
    )
    .context("ServiceInfo::new")?;
    daemon.register(info)?;
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// 浏览局域网内的 bettercursor 实例（~3s 超时）。
pub fn browse_devices(timeout_ms: u64) -> Result<Vec<DiscoveredDevice>> {
    let daemon = mdns_sd::ServiceDaemon::new().context("mdns browse daemon")?;
    let receiver = daemon.browse(SERVICE_TYPE)?;
    let found: Arc<Mutex<Vec<DiscoveredDevice>>> = Arc::new(Mutex::new(Vec::new()));
    let found_clone = Arc::clone(&found);
    std::thread::spawn(move || {
        let deadline =
            std::time::Instant::now() + Duration::from_millis(timeout_ms.max(500));
        while std::time::Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                    let device_id = info
                        .get_properties()
                        .get("device_id")
                        .map(|v| v.val_str())
                        .unwrap_or("")
                        .to_string();
                    let device_name = info
                        .get_properties()
                        .get("device_name")
                        .map(|v| v.val_str())
                        .unwrap_or(&info.get_fullname())
                        .to_string();
                    let host = info
                        .get_addresses()
                        .iter()
                        .next()
                        .map(|a| a.to_string())
                        .unwrap_or_default();
                    let port = info.get_port();
                    if host.is_empty() {
                        continue;
                    }
                    let dev = DiscoveredDevice {
                        device_id: device_id.clone(),
                        device_name,
                        host,
                        port,
                    };
                    let mut guard = found_clone.lock().unwrap();
                    if !guard.iter().any(|d| d.device_id == device_id && d.port == port) {
                        guard.push(dev);
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
    std::thread::sleep(Duration::from_millis(timeout_ms.min(5000)));
    let guard = found.lock().unwrap();
    Ok(guard.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_device_serializes() {
        let d = DiscoveredDevice {
            device_id: "id".into(),
            device_name: "box".into(),
            host: "10.0.0.1".into(),
            port: 38472,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: DiscoveredDevice = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
