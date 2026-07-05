//! Session creation endpoint — persisted in `unified.db`, distinct from
//! `CanonicalSession.sources` (which reflects which layers exist *now*).

use std::collections::HashMap;

use anyhow::Result;

use super::canonical::{CanonicalSession, SourceLayer};
use super::paths;
use super::storage;
use super::unified::UnifiedDb;

/// Load stored origins, infer missing ones, persist first-seen endpoint.
pub fn enrich_created_endpoints(sessions: &mut [CanonicalSession]) -> Result<()> {
    let Ok(unified) = UnifiedDb::open() else {
        infer_only(sessions);
        return Ok(());
    };
    let stored = unified.load_created_origins().unwrap_or_default();
    for s in sessions.iter_mut() {
        if let Some((kind, ms)) = stored.get(&s.uuid) {
            if let Some(ep) = parse_endpoint_kind(kind) {
                s.created_endpoint = Some(ep);
                s.created_at_ms = Some(*ms);
                continue;
            }
        }
        if let Some((ep, ms)) = infer_created_endpoint(s) {
            let kind = endpoint_kind_str(&ep);
            s.created_endpoint = Some(ep);
            s.created_at_ms = Some(ms);
            let _ = unified.persist_created_origin(&s.uuid, kind, ms);
        }
    }
    Ok(())
}

fn infer_only(sessions: &mut [CanonicalSession]) {
    for s in sessions.iter_mut() {
        if s.created_endpoint.is_some() {
            continue;
        }
        if let Some((ep, ms)) = infer_created_endpoint(s) {
            s.created_endpoint = Some(ep);
            s.created_at_ms = Some(ms);
        }
    }
}

/// Infer creation endpoint from on-disk layers (first scan / no unified row).
pub fn infer_created_endpoint(s: &CanonicalSession) -> Option<(SourceLayer, i64)> {
    let has_cli = s.sources.linux_cli.is_some();
    let has_mac = s.sources.mac.is_some();
    let has_ld = s.sources.linux_desktop.is_some();
    let has_desktop = has_mac || has_ld;

    let l2_ms = read_l2_created_at_ms(&s.uuid, &s.project_path);
    let l3_ms = read_l3_created_at_ms(s);

    let desktop_layer = if has_mac {
        SourceLayer::Mac
    } else {
        SourceLayer::LinuxDesktop
    };

    match (has_cli, has_desktop) {
        (false, false) => None,
        (true, false) => {
            let ms = l2_ms.or_else(|| first_positive(&[s.last_updated_at]))?;
            Some((SourceLayer::LinuxCli, ms))
        }
        (false, true) => {
            let ms = l3_ms.or_else(|| first_positive(&[s.last_updated_at]))?;
            Some((desktop_layer, ms))
        }
        (true, true) => resolve_dual_origin(l2_ms, l3_ms, desktop_layer, s),
    }
}

fn resolve_dual_origin(
    l2_ms: Option<i64>,
    l3_ms: Option<i64>,
    desktop_layer: SourceLayer,
    s: &CanonicalSession,
) -> Option<(SourceLayer, i64)> {
    match (l2_ms, l3_ms) {
        (Some(l2), Some(l3)) if l2 < l3 => Some((SourceLayer::LinuxCli, l2)),
        (Some(l2), Some(l3)) if l3 < l2 => Some((desktop_layer, l3)),
        (Some(l2), Some(l3)) => {
            // Same ms — native L2 DAG wins when parseable (CLI-origin dual-stack).
            if super::layer2_messages::read_layer2_turns(&s.uuid, &s.project_path).is_empty() {
                Some((desktop_layer, l3))
            } else {
                Some((SourceLayer::LinuxCli, l2))
            }
        }
        (Some(l2), None) => Some((SourceLayer::LinuxCli, l2)),
        (None, Some(l3)) => Some((desktop_layer, l3)),
        (None, None) => {
            if !super::layer2_messages::read_layer2_turns(&s.uuid, &s.project_path).is_empty() {
                first_positive(&[s.last_updated_at]).map(|ms| (SourceLayer::LinuxCli, ms))
            } else {
                first_positive(&[s.last_updated_at]).map(|ms| (desktop_layer, ms))
            }
        }
    }
}

fn read_l2_created_at_ms(uuid: &str, cwd: &str) -> Option<i64> {
    let store_db = paths::resolve_store_db_for(uuid, cwd)?;
    let r = storage::open_read(&store_db).ok()?;
    let v = r.get_store_meta_json("0").ok()??;
    v.get("createdAt")
        .and_then(|x| x.as_i64())
        .filter(|&t| t > 0)
}

fn read_l3_created_at_ms(s: &CanonicalSession) -> Option<i64> {
    let json = s.composer_data.as_ref().map(|c| c.full_json.as_str())?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let created = v.get("createdAt").and_then(|x| x.as_i64()).unwrap_or(0);
    if created > 0 {
        return Some(created);
    }
    v.get("lastUpdatedAt")
        .and_then(|x| x.as_i64())
        .filter(|&t| t > 0)
}

fn first_positive(vals: &[i64]) -> Option<i64> {
    vals.iter().copied().find(|&t| t > 0)
}

pub fn endpoint_kind_str(layer: &SourceLayer) -> &'static str {
    match layer {
        SourceLayer::Mac => "mac",
        SourceLayer::LinuxCli => "linux_cli",
        SourceLayer::LinuxDesktop => "linux_desktop",
    }
}

pub fn parse_endpoint_kind(s: &str) -> Option<SourceLayer> {
    match s {
        "mac" => Some(SourceLayer::Mac),
        "linux_cli" => Some(SourceLayer::LinuxCli),
        "linux_desktop" => Some(SourceLayer::LinuxDesktop),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::canonical::{ComposerData, SourceInfo, Sources};

    fn sess(uuid: &str, cli: bool, desktop: bool) -> CanonicalSession {
        let mut sources = Sources::default();
        if cli {
            sources.linux_cli = Some(SourceInfo {
                last_seen_at: 0,
                layer: "2".into(),
                path: "/tmp/store.db".into(),
            });
        }
        if desktop {
            sources.linux_desktop = Some(SourceInfo {
                last_seen_at: 0,
                layer: "3".into(),
                path: "/tmp/state.vscdb".into(),
            });
        }
        CanonicalSession {
            uuid: uuid.into(),
            project_slug: "p".into(),
            project_path: "/home/u/proj".into(),
            chat_root: String::new(),
            name: "t".into(),
            last_updated_at: 1000,
            bubble_count: 1,
            is_empty_draft: false,
            is_broken: false,
            broken_reason: None,
            sources,
            first_user_message_preview: String::new(),
            files_referenced: vec![],
            indexable_text: String::new(),
            layer_3_present: desktop,
            layer_3_needs_refresh: false,
            layer_2_needs_refresh: false,
            composer_data: if desktop {
                Some(ComposerData {
                    full_json: r#"{"createdAt":2000,"lastUpdatedAt":3000}"#.into(),
                    subset_json: String::new(),
                })
            } else {
                None
            },
            composer_id: None,
            is_subagent: false,
            subagent_info: None,
            created_endpoint: None,
            created_at_ms: None,
        }
    }

    #[test]
    fn infer_cli_only() {
        let s = sess("u1", true, false);
        let (ep, ms) = infer_created_endpoint(&s).unwrap();
        assert_eq!(ep, SourceLayer::LinuxCli);
        assert_eq!(ms, 1000);
    }

    #[test]
    fn infer_desktop_only_uses_composer_created_at() {
        let s = sess("u2", false, true);
        let (ep, ms) = infer_created_endpoint(&s).unwrap();
        assert_eq!(ep, SourceLayer::LinuxDesktop);
        assert_eq!(ms, 2000);
    }

    #[test]
    fn infer_dual_earlier_l3_is_desktop() {
        let mut s = sess("u3", true, true);
        s.composer_data = Some(ComposerData {
            full_json: r#"{"createdAt":500}"#.into(),
            subset_json: String::new(),
        });
        s.last_updated_at = 900;
        // l2_ms unavailable without real store.db — falls through to l3_ms
        let (ep, ms) = infer_created_endpoint(&s).unwrap();
        assert_eq!(ep, SourceLayer::LinuxDesktop);
        assert_eq!(ms, 500);
    }
}
