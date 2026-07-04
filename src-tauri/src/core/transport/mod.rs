//! bettercursor transport — 跨设备 sync 的传输层 (v0.3.0 async + v4 pull).

use anyhow::Result;
use async_trait::async_trait;

pub mod config;
pub mod snapshot_meta;
pub mod ssh;

pub use config::{PeerConfig, TransportConfigFile};
pub use snapshot_meta::{
    decode_snapshot as decode_snapshot_meta, encode_snapshot as encode_snapshot_meta,
    SessionSnapshot as SessionSnapshotMeta,
};
pub use ssh::SshRsyncTransport;

/// Remote file payload: v4 full snapshot or v0.2.6 metadata-only.
#[derive(Debug, Clone)]
pub enum RemoteSnapshot {
    V4(crate::core::snapshot::SessionSnapshot),
    Meta(SessionSnapshotMeta),
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn push(&self, snap: &SessionSnapshotMeta) -> Result<PushReport>;
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
