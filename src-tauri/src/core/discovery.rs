//! mDNS 局域网发现 — `_bettercursor._tcp` 服务广播与浏览。

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

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
    let daemon = ServiceDaemon::new().context("mdns ServiceDaemon")?;
    let info = build_service_info(device_id, device_name, port)?;
    log::info!(
        "mDNS register: instance={} hostname={} port={}",
        info.get_fullname(),
        info.get_hostname(),
        port
    );
    daemon.register(info)?;
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// 浏览局域网内的 bettercursor 实例（~3s 超时）。
pub fn browse_devices(timeout_ms: u64) -> Result<Vec<DiscoveredDevice>> {
    let daemon = ServiceDaemon::new().context("mdns browse daemon")?;
    let receiver = daemon.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.clamp(500, 5000));
    let mut found = Vec::new();
    while Instant::now() < deadline {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250));
        match receiver.recv_timeout(wait) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if let Some(device) = resolved_device_from_info(&info) {
                    if !found.iter().any(|d: &DiscoveredDevice| {
                        d.device_id == device.device_id && d.port == device.port
                    }) {
                        found.push(device);
                    }
                } else {
                    log::debug!(
                        "mDNS resolved service without address: {}",
                        info.get_fullname()
                    );
                }
            }
            Ok(ServiceEvent::SearchStarted(ty)) => {
                log::debug!("mDNS browse started: {ty}");
            }
            Ok(_) => {}
            Err(_) => continue,
        }
    }
    if let Err(e) = daemon.stop_browse(SERVICE_TYPE) {
        log::debug!("mDNS stop_browse failed: {e}");
    }
    if let Ok(status) = daemon.shutdown() {
        let _ = status.recv_timeout(Duration::from_millis(500));
    }
    log::info!(
        "mDNS browse done: count={} timeout_ms={}",
        found.len(),
        timeout_ms.clamp(500, 5000)
    );
    Ok(filter_out_self_device(
        found,
        &crate::core::device_identity::local_device_id(),
    ))
}

fn build_service_info(device_id: &str, device_name: &str, port: u16) -> Result<ServiceInfo> {
    let host = local_hostname();
    let instance = service_instance_name(device_id);
    let mut props = HashMap::new();
    props.insert("device_id".into(), device_id.to_string());
    props.insert("device_name".into(), device_name.to_string());
    ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &format!("{host}.local."),
        "",
        port,
        Some(props),
    )
    .map(ServiceInfo::enable_addr_auto)
    .context("ServiceInfo::new")
}

fn resolved_device_from_info(info: &ServiceInfo) -> Option<DiscoveredDevice> {
    let addr = info
        .get_addresses()
        .iter()
        .find(|addr| matches!(addr, std::net::IpAddr::V4(v4) if !v4.is_loopback()))
        .or_else(|| info.get_addresses().iter().find(|addr| addr.is_ipv4()))
        .or_else(|| {
            info.get_addresses()
                .iter()
                .find(|addr| matches!(addr, std::net::IpAddr::V6(v6) if !v6.is_loopback()))
        })
        .or_else(|| info.get_addresses().iter().next())?;
    let host = match addr {
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        _ => addr.to_string(),
    };
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
        .unwrap_or(info.get_fullname())
        .to_string();
    Some(DiscoveredDevice {
        device_id,
        device_name,
        host,
        port: info.get_port(),
    })
}

fn service_instance_name(device_id: &str) -> String {
    let short_id: String = device_id.chars().take(8).collect();
    if short_id.is_empty() {
        "bettercursor".to_string()
    } else {
        format!("bettercursor-{short_id}")
    }
}

fn filter_out_self_device(
    devices: Vec<DiscoveredDevice>,
    self_device_id: &str,
) -> Vec<DiscoveredDevice> {
    if self_device_id.trim().is_empty() {
        return devices;
    }
    devices
        .into_iter()
        .filter(|device| device.device_id != self_device_id)
        .collect()
}

fn local_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bettercursor".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    #[test]
    fn build_service_info_enables_addr_auto() {
        let info = build_service_info("device-12345678", "box", 38472).unwrap();
        assert!(info.is_addr_auto());
        assert_eq!(info.get_port(), 38472);
        assert_eq!(
            info.get_property_val_str("device_id"),
            Some("device-12345678")
        );
        assert_eq!(info.get_property_val_str("device_name"), Some("box"));
        assert!(info.get_fullname().starts_with("bettercursor-device-1."));
    }

    #[test]
    fn resolved_device_uses_first_address_and_properties() {
        let mut props = HashMap::new();
        props.insert("device_id".to_string(), "dev-1".to_string());
        props.insert("device_name".to_string(), "workstation".to_string());
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "bettercursor-dev-1",
            "test-host.local.",
            "127.0.0.1,192.168.1.9",
            38472,
            Some(props),
        )
        .unwrap();
        let device = resolved_device_from_info(&info).unwrap();
        assert_eq!(device.device_id, "dev-1");
        assert_eq!(device.device_name, "workstation");
        assert_eq!(device.host, "192.168.1.9");
        assert_eq!(device.port, 38472);
    }

    #[test]
    fn filter_out_self_device_removes_local_id() {
        let devices = vec![
            DiscoveredDevice {
                device_id: "self".into(),
                device_name: "my-mac".into(),
                host: "192.168.1.2".into(),
                port: 38472,
            },
            DiscoveredDevice {
                device_id: "peer".into(),
                device_name: "linux-box".into(),
                host: "192.168.1.3".into(),
                port: 38472,
            },
        ];
        let filtered = filter_out_self_device(devices, "self");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].device_id, "peer");
    }
}
