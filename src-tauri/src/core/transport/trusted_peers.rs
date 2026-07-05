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
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
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
}
