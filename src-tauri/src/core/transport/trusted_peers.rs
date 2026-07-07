//! 已配对可信设备 — LAN 开箱即用路径的 peer 存储。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustedPeer {
    pub id: String,
    pub device_name: String,
    /// `host:port` for LAN TCP (e.g. `192.168.1.10:38472`).
    pub lan_addr: String,
    /// Shared secret from pairing (hex).
    pub pairing_secret: String,
    pub trusted_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrustedPeersFile {
    #[serde(default)]
    pub peers: Vec<TrustedPeer>,
}

impl TrustedPeersFile {
    pub fn path() -> PathBuf {
        crate::core::paths::bettercursor_dir().join("trusted_peers.json")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let body =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        Ok(serde_json::from_str(&body).context("parse trusted_peers.json")?)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, &body)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn peer(&self, id: &str) -> Option<&TrustedPeer> {
        self.peers.iter().find(|p| p.id == id)
    }

    pub fn upsert(&mut self, peer: TrustedPeer) {
        if let Some(existing) = self.peers.iter_mut().find(|p| p.id == peer.id) {
            *existing = peer;
        } else {
            self.peers.push(peer);
        }
    }

    /// 修复旧版错误写入的设备名 / peer id，并尽量用 discovery 结果对齐。
    pub fn cleanup_with_discovery(
        &mut self,
        discovered: &[crate::core::discovery::DiscoveredDevice],
        local_device_id: &str,
        local_device_name: &str,
    ) -> Result<bool> {
        let mut changed = false;
        let mut migrations = Vec::new();
        for peer in &mut self.peers {
            let before = peer.clone();
            if peer.device_name.trim().is_empty()
                || (peer.device_name == local_device_name && peer.id != local_device_id)
            {
                peer.device_name = fallback_device_name(&peer.lan_addr);
            }
            if let Some(device) = discovered
                .iter()
                .find(|device| matches_device(peer, device))
            {
                if !device.device_id.trim().is_empty() {
                    peer.id = device.device_id.clone();
                }
                peer.device_name = device.device_name.clone();
                peer.lan_addr = format!("{}:{}", device.host, device.port);
            }
            if *peer != before {
                changed = true;
                if before.id != peer.id {
                    migrations.push((before.id, peer.id.clone()));
                }
            }
        }
        if dedupe_peers(&mut self.peers) {
            changed = true;
        }
        if changed {
            for (old_id, new_id) in migrations {
                crate::core::transport::outbox::rekey_peer_dir(&old_id, &new_id)?;
            }
        }
        Ok(changed)
    }
}

fn matches_device(peer: &TrustedPeer, device: &crate::core::discovery::DiscoveredDevice) -> bool {
    let discovered_addr = format!("{}:{}", device.host, device.port);
    peer.id == device.device_id || peer.lan_addr == discovered_addr
}

pub(crate) fn fallback_device_name(lan_addr: &str) -> String {
    lan_addr
        .rsplit_once(':')
        .map(|(host, _)| host.to_string())
        .filter(|host| !host.trim().is_empty())
        .unwrap_or_else(|| lan_addr.to_string())
}

fn dedupe_peers(peers: &mut Vec<TrustedPeer>) -> bool {
    let before = peers.clone();
    let mut deduped = Vec::new();
    for peer in std::mem::take(peers) {
        if let Some(existing) = deduped
            .iter_mut()
            .find(|p: &&mut TrustedPeer| p.id == peer.id || p.lan_addr == peer.lan_addr)
        {
            merge_peer(existing, peer);
        } else {
            deduped.push(peer);
        }
    }
    *peers = deduped;
    *peers != before
}

fn merge_peer(existing: &mut TrustedPeer, incoming: TrustedPeer) {
    if incoming.trusted_at_ms >= existing.trusted_at_ms {
        existing.id = incoming.id;
        existing.lan_addr = incoming.lan_addr;
        existing.pairing_secret = incoming.pairing_secret;
        existing.trusted_at_ms = incoming.trusted_at_ms;
    }
    if !incoming.device_name.trim().is_empty() {
        existing.device_name = incoming.device_name;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_peer_round_trip() {
        let p = TrustedPeer {
            id: "dev-1".into(),
            device_name: "linux-box".into(),
            lan_addr: "192.168.0.5:38472".into(),
            pairing_secret: "abc123".into(),
            trusted_at_ms: 1,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: TrustedPeer = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn cleanup_replaces_legacy_self_name_with_addr_fallback() {
        let mut peers = TrustedPeersFile {
            peers: vec![TrustedPeer {
                id: "legacy".into(),
                device_name: "my-mac".into(),
                lan_addr: "192.168.0.5:38472".into(),
                pairing_secret: "abc123".into(),
                trusted_at_ms: 1,
            }],
        };
        let changed = peers
            .cleanup_with_discovery(&[], "self-id", "my-mac")
            .unwrap();
        assert!(changed);
        assert_eq!(peers.peers[0].device_name, "192.168.0.5");
    }

    #[test]
    fn cleanup_reconciles_discovered_name_and_id() {
        let mut peers = TrustedPeersFile {
            peers: vec![TrustedPeer {
                id: "legacy-random".into(),
                device_name: "192.168.0.5".into(),
                lan_addr: "192.168.0.5:38472".into(),
                pairing_secret: "abc123".into(),
                trusted_at_ms: 1,
            }],
        };
        let discovered = vec![crate::core::discovery::DiscoveredDevice {
            device_id: "stable-peer-id".into(),
            device_name: "macbook-pro".into(),
            host: "192.168.0.5".into(),
            port: 38472,
        }];
        let changed = peers
            .cleanup_with_discovery(&discovered, "self-id", "my-mac")
            .unwrap();
        assert!(changed);
        assert_eq!(peers.peers[0].id, "stable-peer-id");
        assert_eq!(peers.peers[0].device_name, "macbook-pro");
    }
}
