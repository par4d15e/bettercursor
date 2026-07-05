//! bettercursor transport — 跨设备 sync 的传输层 (v0.3.0 async + v4 pull).

use anyhow::Result;
use async_trait::async_trait;

pub mod config;
pub mod lan;
pub mod outbox;
pub mod snapshot_meta;
pub mod ssh;
pub mod trusted_peers;

pub use config::{PeerConfig, TransportConfigFile};
pub use lan::LanTcpTransport;
pub use snapshot_meta::{
    decode_snapshot as decode_snapshot_meta, encode_snapshot as encode_snapshot_meta,
    SessionSnapshot as SessionSnapshotMeta,
};
pub use ssh::SshRsyncTransport;
pub use trusted_peers::TrustedPeersFile;

/// Payload for push — v4 full snapshot (default) or v0.2.6 metadata-only.
#[derive(Debug, Clone)]
pub enum PushSnapshot {
    V4(crate::core::snapshot::SessionSnapshot),
    Meta(SessionSnapshotMeta),
}

impl PushSnapshot {
    pub fn uuid(&self) -> &str {
        match self {
            Self::V4(s) => s.composer.composer_id.as_str(),
            Self::Meta(m) => m.uuid.as_str(),
        }
    }

    pub fn host_namespace(&self) -> &str {
        match self {
            Self::V4(s) => s.source_endpoint.host.as_str(),
            Self::Meta(m) => m.host.as_str(),
        }
    }

    pub fn encode_body(&self) -> Result<String> {
        match self {
            Self::V4(s) => crate::core::snapshot::encode_snapshot(s),
            Self::Meta(m) => encode_snapshot_meta(m).map_err(Into::into),
        }
    }
}

/// Remote file payload: v4 full snapshot or v0.2.6 metadata-only.
#[derive(Debug, Clone)]
pub enum RemoteSnapshot {
    V4(crate::core::snapshot::SessionSnapshot),
    Meta(SessionSnapshotMeta),
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn push(&self, snap: &PushSnapshot) -> Result<PushReport>;
    async fn pull(&self, since_ms: i64) -> Result<Vec<RemoteSnapshot>>;
    async fn list_remote(&self) -> Result<Vec<RemoteSessionMeta>>;
    fn endpoint_id(&self) -> &str;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PushReport {
    pub uuid: String,
    pub bytes_written: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteSessionMeta {
    pub uuid: String,
    pub host: String,
    pub last_updated_at_ms: i64,
    pub project_slug: String,
    pub source_path: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerSummary {
    pub id: String,
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub identity_file: String,
    pub remote_snap_dir: String,
    pub remote_hostname: String,
}

impl PeerSummary {
    pub fn from(c: config::PeerConfig) -> Self {
        Self {
            id: c.id,
            kind: c.kind,
            host: c.host,
            port: c.port,
            identity_file: c.identity_file,
            remote_snap_dir: c.remote_snap_dir,
            remote_hostname: c.remote_hostname,
        }
    }
}

/// 按 peer_id 解析：优先 `trusted_peers.json` (LAN)，否则 `transports.json` (SSH)。
#[derive(Debug, Clone)]
pub enum ResolvedPeer {
    Ssh(PeerConfig),
    Lan(trusted_peers::TrustedPeer),
}

pub fn resolve_peer(peer_id: &str) -> Result<ResolvedPeer> {
    if let Ok(tp) = trusted_peers::TrustedPeersFile::load() {
        if let Some(p) = tp.peers.iter().find(|p| p.id == peer_id) {
            return Ok(ResolvedPeer::Lan(p.clone()));
        }
    }
    let cfg = TransportConfigFile::load()?;
    let peer = cfg
        .peer(peer_id)
        .ok_or_else(|| anyhow::anyhow!("peer '{peer_id}' not found"))?
        .clone();
    Ok(ResolvedPeer::Ssh(peer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_report_serializes_all_fields() {
        let r = PushReport {
            uuid: "abc-123".into(),
            bytes_written: 512,
            duration_ms: 42,
        };
        let json: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(json["uuid"], "abc-123");
        assert_eq!(json["bytes_written"], 512);
        assert_eq!(json["duration_ms"], 42);
    }

    #[test]
    fn push_snapshot_v4_encode_round_trip() {
        use crate::core::snapshot::{decode_snapshot, ComposerMeta, SessionSnapshot, SourceEndpoint};
        let snap = SessionSnapshot {
            version: 4,
            exported_at: 1,
            source_endpoint: SourceEndpoint {
                host: "host-a".into(),
                os: "linux".into(),
                user: "eric".into(),
                endpoint_kind: "linux_desktop".into(),
                cursor_version: None,
            },
            composer: ComposerMeta {
                composer_id: "uuid-1".into(),
                last_updated_at: 1,
                project_path: "/p".into(),
                project_slug: "slug".into(),
                workspace_id: "ws".into(),
                chat_root: "/p".into(),
            },
            bubbles: vec![],
            blob_refs: vec![],
            raw_blobs: std::collections::HashMap::new(),
        };
        let payload = PushSnapshot::V4(snap.clone());
        assert_eq!(payload.uuid(), "uuid-1");
        let body = payload.encode_body().unwrap();
        let back = decode_snapshot(&body).unwrap();
        assert_eq!(back.composer.composer_id, snap.composer.composer_id);
    }

    #[test]
    fn remote_session_meta_round_trip() {
        let m = RemoteSessionMeta {
            uuid: "uuid-xyz".into(),
            host: "macbook-pro-m1".into(),
            last_updated_at_ms: 1_700_000_000_000,
            project_slug: "enenzuo".into(),
            source_path: "/Users/eric/.cursor/projects/foo/bar.jsonl".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: RemoteSessionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(m.uuid, back.uuid);
        assert_eq!(m.host, back.host);
    }
}
