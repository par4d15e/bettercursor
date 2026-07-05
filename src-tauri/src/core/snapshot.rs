//! v0.3.0 snapshot codec v4 — bubble-level plain JSON (SYNC_DESIGN §2).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::canonical::{Bubble, CanonicalSession};

pub const SNAPSHOT_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionSnapshot {
    pub version: u32,
    pub exported_at: i64,
    pub source_endpoint: SourceEndpoint,
    pub composer: ComposerMeta,
    pub bubbles: Vec<SnapshotBubble>,
    #[serde(default)]
    pub blob_refs: Vec<String>,
    #[serde(default)]
    pub raw_blobs: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceEndpoint {
    pub host: String,
    pub os: String,
    pub user: String,
    pub endpoint_kind: String,
    #[serde(default)]
    pub cursor_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComposerMeta {
    pub composer_id: String,
    pub last_updated_at: i64,
    pub project_path: String,
    pub project_slug: String,
    pub workspace_id: String,
    pub chat_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotBubble {
    pub id: String,
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub tool_calls: Vec<SnapshotToolUse>,
    #[serde(default)]
    pub files: Vec<String>,
    pub ts_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_bubble_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotToolUse {
    pub name: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

impl SessionSnapshot {
    pub fn from_canonical_v4(
        session: &CanonicalSession,
        bubbles: &[Bubble],
        host: &str,
        exported_at_ms: i64,
    ) -> Self {
        let endpoint_kind = session.sources.preferred_endpoint_kind();
        let os = std::env::consts::OS.to_string();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());

        let workspace_id = session
            .composer_data
            .as_ref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c.full_json).ok())
            .and_then(|v| {
                v.get("workspaceIdentifier")
                    .and_then(|wi| wi.get("id"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "empty-window".to_string());

        Self {
            version: SNAPSHOT_VERSION,
            exported_at: exported_at_ms,
            source_endpoint: SourceEndpoint {
                host: host.to_string(),
                os,
                user,
                endpoint_kind: endpoint_kind.to_string(),
                cursor_version: None,
            },
            composer: ComposerMeta {
                composer_id: session.composer_id.clone().unwrap_or_else(|| session.uuid.clone()),
                last_updated_at: session.last_updated_at,
                project_path: session.project_path.clone(),
                project_slug: session.project_slug.clone(),
                workspace_id,
                chat_root: session.chat_root.clone(),
            },
            bubbles: bubbles.iter().map(SnapshotBubble::from_canonical).collect(),
            blob_refs: Vec::new(),
            raw_blobs: HashMap::new(),
        }
    }

    pub fn content_hash(&self) -> String {
        let canonical: Vec<Bubble> = self.bubbles.iter().map(Bubble::from_snapshot).collect();
        crate::core::conflict::content_hash_from_bubbles(&canonical)
    }
}

impl SnapshotBubble {
    fn from_canonical(b: &Bubble) -> Self {
        Self {
            id: b.id.clone(),
            role: b.role.clone(),
            text: b.text.clone(),
            tool_calls: b
                .tool_calls
                .iter()
                .map(|t| SnapshotToolUse {
                    name: t.name.clone(),
                    input: t.input.clone().unwrap_or(serde_json::Value::Null),
                })
                .collect(),
            files: b.files.clone(),
            ts_ms: b.created_at_ms,
            parent_bubble_id: b.parent_bubble_id.clone(),
        }
    }
}

impl Bubble {
    pub fn from_snapshot(b: &SnapshotBubble) -> Self {
        Self {
            id: b.id.clone(),
            role: b.role.clone(),
            text: b.text.clone(),
            tool_calls: b
                .tool_calls
                .iter()
                .map(|t| crate::core::canonical::BubbleToolUse {
                    name: t.name.clone(),
                    input: if t.input.is_null() {
                        None
                    } else {
                        Some(t.input.clone())
                    },
                })
                .collect(),
            files: b.files.clone(),
            images: Vec::new(),
            created_at_ms: b.ts_ms,
            parent_bubble_id: b.parent_bubble_id.clone(),
        }
    }
}

pub fn encode_snapshot(s: &SessionSnapshot) -> Result<String> {
    Ok(serde_json::to_string(s)?)
}

pub fn decode_snapshot(json: &str) -> Result<SessionSnapshot> {
    let snap: SessionSnapshot = serde_json::from_str(json)?;
    if snap.version != SNAPSHOT_VERSION {
        anyhow::bail!(
            "unsupported snapshot version {} (expected {})",
            snap.version,
            SNAPSHOT_VERSION
        );
    }
    Ok(snap)
}

/// Atomic write: tmp file + rename into `out_dir/<host>/<uuid>-<exported_at>.json`.
pub fn write_snapshot_file(
    out_dir: &Path,
    host: &str,
    snap: &SessionSnapshot,
) -> Result<PathBuf> {
    let host_dir = out_dir.join(host);
    std::fs::create_dir_all(&host_dir)?;
    let fname = format!("{}-{}.json", snap.composer.composer_id, snap.exported_at);
    let final_path = host_dir.join(&fname);
    let tmp_path = host_dir.join(format!("{fname}.tmp"));
    let body = encode_snapshot(snap)?;
    std::fs::write(&tmp_path, &body)?;
    std::fs::rename(&tmp_path, &final_path).with_context(|| {
        format!(
            "atomic rename {} → {}",
            tmp_path.display(),
            final_path.display()
        )
    })?;
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::canonical::{CanonicalSession, SourceInfo, Sources};

    fn minimal_session() -> CanonicalSession {
        CanonicalSession {
            uuid: "uuid-v4".into(),
            project_slug: "proj".into(),
            project_path: "/tmp/proj".into(),
            chat_root: "abc".into(),
            name: "n".into(),
            last_updated_at: 1_700_000_000_000,
            bubble_count: 1,
            is_empty_draft: false,
            is_broken: false,
            broken_reason: None,
            sources: Sources::default(),
            first_user_message_preview: "hi".into(),
            files_referenced: vec![],
            indexable_text: String::new(),
            layer_3_present: true,
            layer_3_needs_refresh: false,
            layer_2_needs_refresh: false,
            created_endpoint: None,
            created_at_ms: None,
            composer_data: None,
            composer_id: Some("uuid-v4".into()),
            is_subagent: false,
            subagent_info: None,
        }
    }

    #[test]
    fn encode_decode_v4_round_trip() {
        let session = minimal_session();
        let bubbles = vec![Bubble {
            id: "b1".into(),
            role: "user".into(),
            text: "hello".into(),
            tool_calls: vec![],
            files: vec![],
            images: vec![],
            created_at_ms: 1000,
            parent_bubble_id: None,
        }];
        let snap = SessionSnapshot::from_canonical_v4(&session, &bubbles, "host-a", 1_700_000_001_000);
        let json = encode_snapshot(&snap).unwrap();
        let back = decode_snapshot(&json).unwrap();
        assert_eq!(snap, back);
        assert_eq!(back.bubbles[0].ts_ms, 1000);
    }

    #[test]
    fn from_canonical_maps_ts_ms() {
        let session = minimal_session();
        let bubbles = vec![Bubble {
            id: "b1".into(),
            role: "assistant".into(),
            text: "x".into(),
            tool_calls: vec![],
            files: vec![],
            images: vec![],
            created_at_ms: 42,
            parent_bubble_id: None,
        }];
        let snap = SessionSnapshot::from_canonical_v4(&session, &bubbles, "h", 1);
        assert_eq!(snap.bubbles[0].ts_ms, 42);
        assert_eq!(snap.version, 4);
    }

    #[test]
    fn write_snapshot_file_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let session = minimal_session();
        let snap = SessionSnapshot::from_canonical_v4(&session, &[], "myhost", 99);
        let path = write_snapshot_file(dir.path(), "myhost", &snap).unwrap();
        assert!(path.exists());
        assert!(!dir.path().join("myhost").join("uuid-v4-99.json.tmp").exists());
    }

    #[test]
    fn decode_rejects_wrong_version() {
        let bad = r#"{"version":3,"exported_at":1,"source_endpoint":{"host":"h","os":"linux","user":"u","endpoint_kind":"linux_cli"},"composer":{"composer_id":"x","last_updated_at":1,"project_path":"","project_slug":"","workspace_id":"","chat_root":""},"bubbles":[]}"#;
        assert!(decode_snapshot(bad).is_err());
    }
}
