//! v0.3.6: shared transport pull → unified.db upsert → L2/L3 apply.

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use super::conflict::{self, ConflictClass};
use super::snapshot::SessionSnapshot;
use super::transport::{RemoteSnapshot, ResolvedPeer, Transport};
use super::{canonical, path_rewrite, sync, unified};

#[derive(Debug, Clone, Serialize)]
pub struct SessionPullResult {
    pub uuid: String,
    pub conflict_class: String,
    pub applied_l2: bool,
    pub applied_l3: bool,
    pub skipped: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullReport {
    pub peer_id: String,
    pub count: usize,
    pub snapshots: Vec<super::transport::RemoteSessionMeta>,
    pub results: Vec<SessionPullResult>,
    pub applied: usize,
    pub skipped_count: usize,
    pub failed: usize,
}

pub fn pull_snapshots_from_peer(peer_id: &str, since_ms: i64) -> Result<Vec<RemoteSnapshot>> {
    let resolved = super::transport::resolve_peer(peer_id)?;
    match &resolved {
        ResolvedPeer::Ssh(peer) => {
            let transport = super::transport::SshRsyncTransport::new(peer.clone());
            tauri::async_runtime::block_on(transport.pull(since_ms))
        }
        ResolvedPeer::Lan(peer) => {
            let transport = super::transport::LanTcpTransport::new(peer.clone());
            tauri::async_runtime::block_on(transport.pull(since_ms))
        }
    }
}

/// Pull from peer, classify conflicts, upsert unified.db, apply L2/L3 when needed.
pub fn pull_and_apply_from_peer(peer_id: &str, since_ms: i64) -> Result<PullReport> {
    let snaps = pull_snapshots_from_peer(peer_id, since_ms)?;
    let unified = unified::UnifiedDb::open()?;
    let started_ms = chrono::Utc::now().timestamp_millis();
    let run_id = unified.record_sync_run(peer_id, "pull", started_ms)?;

    let mut processed = 0u32;
    let mut failed_db = 0u32;
    let mut pull_error: Option<String> = None;
    let mut results = Vec::new();
    let mut applied = 0usize;
    let mut skipped_count = 0usize;
    let mut failed = 0usize;

    for snap in &snaps {
        let RemoteSnapshot::V4(v4) = snap else {
            continue;
        };
        let uuid = v4.composer.composer_id.clone();
        let class = classify_v4(v4, &unified)?;
        let session_result = apply_v4_snapshot(v4, class, &unified);

        match &session_result {
            Ok(r) => {
                processed += 1;
                if r.applied_l2 || r.applied_l3 {
                    applied += 1;
                } else if r.error.is_none() && r.skipped.is_empty() {
                    skipped_count += 1;
                } else if r.error.is_some() {
                    failed += 1;
                } else if !r.skipped.is_empty() {
                    skipped_count += 1;
                }
                results.push(r.clone());
            }
            Err(e) => {
                failed_db += 1;
                failed += 1;
                log::warn!("transport_pull apply failed for {uuid}: {e:#}");
                if pull_error.is_none() {
                    pull_error = Some(format!("{uuid}: {e:#}"));
                }
                results.push(SessionPullResult {
                    uuid: uuid.clone(),
                    conflict_class: conflict_class_label(class),
                    applied_l2: false,
                    applied_l3: false,
                    skipped: vec![],
                    error: Some(format!("{e:#}")),
                });
            }
        }
    }

    let finished_ms = chrono::Utc::now().timestamp_millis();
    unified.finish_sync_run(
        run_id,
        processed,
        failed_db,
        finished_ms,
        pull_error.as_deref(),
    )?;

    let snapshots: Vec<super::transport::RemoteSessionMeta> = snaps
        .iter()
        .filter_map(|s| match s {
            RemoteSnapshot::V4(v) => Some(super::transport::RemoteSessionMeta {
                uuid: v.composer.composer_id.clone(),
                host: v.source_endpoint.host.clone(),
                last_updated_at_ms: v.composer.last_updated_at,
                project_slug: v.composer.project_slug.clone(),
                source_path: v.composer.project_path.clone(),
            }),
            RemoteSnapshot::Meta(m) => Some(super::transport::RemoteSessionMeta {
                uuid: m.uuid.clone(),
                host: m.host.clone(),
                last_updated_at_ms: m.last_updated_at_ms,
                project_slug: m.project_slug.clone(),
                source_path: m.source_path.clone(),
            }),
        })
        .collect();
    let count = snapshots.len();

    Ok(PullReport {
        peer_id: peer_id.to_string(),
        count,
        snapshots,
        results,
        applied,
        skipped_count,
        failed,
    })
}

fn classify_v4(v4: &SessionSnapshot, unified: &unified::UnifiedDb) -> Result<ConflictClass> {
    let uuid = &v4.composer.composer_id;
    let incoming_hash = v4.content_hash();
    let incoming_updated = v4.composer.last_updated_at;
    let local_meta = unified.get_session_meta(uuid)?;
    let local_hash = local_meta.as_ref().map(|m| m.content_hash.as_str());
    let local_updated = local_meta
        .as_ref()
        .map(|m| m.last_updated_at_ms)
        .unwrap_or(0);
    Ok(conflict::classify(
        local_hash,
        local_updated,
        &incoming_hash,
        incoming_updated,
    ))
}

fn apply_v4_snapshot(
    v4: &SessionSnapshot,
    class: ConflictClass,
    unified: &unified::UnifiedDb,
) -> Result<SessionPullResult> {
    let uuid = v4.composer.composer_id.clone();
    let remote_path = v4.composer.project_path.clone();
    let project_slug = v4.composer.project_slug.clone();
    let incoming_hash = v4.content_hash();
    let local_meta = unified.get_session_meta(&uuid)?;
    let local_hash = local_meta.as_ref().map(|m| m.content_hash.as_str());
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut bubbles_to_apply: Option<Vec<canonical::Bubble>> = None;

    match class {
        ConflictClass::LocalAhead => {
            return Ok(SessionPullResult {
                uuid,
                conflict_class: conflict_class_label(class),
                applied_l2: false,
                applied_l3: false,
                skipped: vec!["local_ahead".into()],
                error: None,
            });
        }
        ConflictClass::New | ConflictClass::IncomingNewer => {
            if local_meta.is_some() {
                let rows = unified.get_bubbles(&uuid)?;
                let local_bubbles = unified::UnifiedDb::bubbles_from_rows(&rows);
                let payload = conflict::auto_archive_before_overwrite(&local_bubbles);
                unified.record_archive(&uuid, "before_overwrite", &payload, now_ms)?;
            }
            let incoming: Vec<canonical::Bubble> = v4
                .bubbles
                .iter()
                .map(canonical::Bubble::from_snapshot)
                .collect();
            unified.upsert_session_from_snapshot(v4, &incoming, now_ms)?;
            bubbles_to_apply = Some(incoming);
        }
        ConflictClass::Identical => {
            let incoming: Vec<canonical::Bubble> = v4
                .bubbles
                .iter()
                .map(canonical::Bubble::from_snapshot)
                .collect();
            unified.upsert_session_from_snapshot(v4, &incoming, now_ms)?;
            let local_cwd = path_rewrite::rewrite_project_path(&remote_path, &project_slug);
            let need_apply = !sync::layer2_is_fully_synced(&uuid, &local_cwd)
                || !sync::layer3_is_fully_synced(&uuid, &local_cwd);
            if need_apply {
                bubbles_to_apply = Some(incoming);
            }
        }
        ConflictClass::Diverged => {
            let local_bubbles = if local_meta.is_some() {
                let rows = unified.get_bubbles(&uuid)?;
                unified::UnifiedDb::bubbles_from_rows(&rows)
            } else {
                Vec::new()
            };
            let incoming: Vec<canonical::Bubble> = v4
                .bubbles
                .iter()
                .map(canonical::Bubble::from_snapshot)
                .collect();
            let (merged, archive_payload) = conflict::auto_merge(&local_bubbles, &incoming);
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
        let apply = sync::apply_session_from_snapshot(
            &uuid,
            &remote_path,
            &project_slug,
            &bubbles,
            &v4.raw_blobs,
        )?;
        return Ok(SessionPullResult {
            uuid,
            conflict_class: conflict_class_label(class),
            applied_l2: apply.wrote_layer2,
            applied_l3: apply.wrote_layer3,
            skipped: apply.skipped,
            error: None,
        });
    }

    Ok(SessionPullResult {
        uuid,
        conflict_class: conflict_class_label(class),
        applied_l2: false,
        applied_l3: false,
        skipped: vec![],
        error: None,
    })
}

fn conflict_class_label(class: ConflictClass) -> String {
    match class {
        ConflictClass::New => "new",
        ConflictClass::Identical => "identical",
        ConflictClass::IncomingNewer => "incoming_newer",
        ConflictClass::LocalAhead => "local_ahead",
        ConflictClass::Diverged => "diverged",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_class_label_round_trip() {
        assert_eq!(conflict_class_label(ConflictClass::Identical), "identical");
    }
}
