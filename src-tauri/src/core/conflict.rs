//! v0.3.0 conflict classification — 5-way enum + bubble diff + auto_merge.
//!
//! Ported from `bettercursor/conflict.py` + SYNC_DESIGN §6.

use crate::core::canonical::Bubble;
use serde_json::json;
use sha2::{Digest, Sha256};

/// Five-way conflict classification (PascalCase in DB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictClass {
    New,
    Identical,
    IncomingNewer,
    LocalAhead,
    Diverged,
}

impl ConflictClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Identical => "Identical",
            Self::IncomingNewer => "IncomingNewer",
            Self::LocalAhead => "LocalAhead",
            Self::Diverged => "Diverged",
        }
    }
}

/// Hash-based 5-way classify (SYNC_DESIGN §6.2).
pub fn classify(
    local_hash: Option<&str>,
    local_updated_ms: i64,
    incoming_hash: &str,
    incoming_updated_ms: i64,
) -> ConflictClass {
    let local_hash = local_hash.unwrap_or("");
    if local_hash.is_empty() {
        return ConflictClass::New;
    }
    if local_hash == incoming_hash {
        return ConflictClass::Identical;
    }
    if incoming_updated_ms > local_updated_ms {
        return ConflictClass::IncomingNewer;
    }
    if local_updated_ms > incoming_updated_ms {
        return ConflictClass::LocalAhead;
    }
    ConflictClass::Diverged
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BubbleDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
}

pub fn bubble_diff(local: &[Bubble], incoming: &[Bubble]) -> BubbleDiff {
    use std::collections::{HashMap, HashSet};
    let local_map: HashMap<&str, &Bubble> = local.iter().map(|b| (b.id.as_str(), b)).collect();
    let incoming_map: HashMap<&str, &Bubble> = incoming.iter().map(|b| (b.id.as_str(), b)).collect();
    let local_ids: HashSet<&str> = local_map.keys().copied().collect();
    let incoming_ids: HashSet<&str> = incoming_map.keys().copied().collect();

    let added: Vec<String> = incoming_ids
        .difference(&local_ids)
        .map(|s| (*s).to_string())
        .collect();
    let removed: Vec<String> = local_ids
        .difference(&incoming_ids)
        .map(|s| (*s).to_string())
        .collect();
    let mut modified = Vec::new();
    for id in local_ids.intersection(&incoming_ids) {
        let l = local_map[id];
        let i = incoming_map[id];
        if l.text != i.text
            || l.role != i.role
            || l.created_at_ms != i.created_at_ms
            || l.tool_calls != i.tool_calls
        {
            modified.push((*id).to_string());
        }
    }
    BubbleDiff {
        added,
        removed,
        modified,
    }
}

pub fn content_hash_from_bubbles(bubbles: &[Bubble]) -> String {
    let mut sorted: Vec<&Bubble> = bubbles.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let payload = serde_json::to_string(&sorted.iter().map(|b| {
        json!({
            "id": b.id,
            "role": b.role,
            "text": b.text,
            "ts": b.created_at_ms,
        })
    }).collect::<Vec<_>>()).unwrap_or_default();
    let digest = Sha256::digest(payload.as_bytes());
    hex::encode(digest)
}

/// Incoming-first merge: incoming wins on overlap; local-only bubbles appended.
pub fn auto_merge(local: &[Bubble], incoming: &[Bubble]) -> (Vec<Bubble>, String) {
    use std::collections::HashMap;
    let mut by_id: HashMap<String, Bubble> = HashMap::new();
    for b in local {
        by_id.insert(b.id.clone(), b.clone());
    }
    for b in incoming {
        by_id.insert(b.id.clone(), b.clone());
    }
    let mut merged: Vec<Bubble> = by_id.into_values().collect();
    merged.sort_by(|a, b| {
        a.created_at_ms
            .cmp(&b.created_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    let archive = serde_json::to_string(local).unwrap_or_else(|_| "[]".to_string());
    (merged, archive)
}

pub fn auto_archive_before_overwrite(local_bubbles: &[Bubble]) -> String {
    serde_json::to_string(local_bubbles).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bubble(id: &str, text: &str, ts: i64) -> Bubble {
        Bubble {
            id: id.into(),
            role: "user".into(),
            text: text.into(),
            tool_calls: vec![],
            files: vec![],
            images: vec![],
            created_at_ms: ts,
            parent_bubble_id: None,
        }
    }

    #[test]
    fn classify_table_new() {
        assert_eq!(
            classify(None, 0, "abc", 100),
            ConflictClass::New
        );
    }

    #[test]
    fn classify_table_identical() {
        assert_eq!(
            classify(Some("h"), 100, "h", 50),
            ConflictClass::Identical
        );
    }

    #[test]
    fn classify_table_incoming_newer() {
        assert_eq!(
            classify(Some("a"), 100, "b", 200),
            ConflictClass::IncomingNewer
        );
    }

    #[test]
    fn classify_table_local_ahead() {
        assert_eq!(
            classify(Some("a"), 300, "b", 100),
            ConflictClass::LocalAhead
        );
    }

    #[test]
    fn classify_table_diverged() {
        assert_eq!(
            classify(Some("a"), 100, "b", 100),
            ConflictClass::Diverged
        );
    }

    #[test]
    fn bubble_diff_added_removed_modified() {
        let local = vec![bubble("a", "one", 1), bubble("b", "two", 2)];
        let incoming = vec![bubble("a", "one-changed", 1), bubble("c", "three", 3)];
        let d = bubble_diff(&local, &incoming);
        assert_eq!(d.added, vec!["c"]);
        assert_eq!(d.removed, vec!["b"]);
        assert_eq!(d.modified, vec!["a"]);
    }

    #[test]
    fn content_hash_deterministic() {
        let b = vec![bubble("x", "hi", 1)];
        assert_eq!(content_hash_from_bubbles(&b), content_hash_from_bubbles(&b));
    }

    #[test]
    fn auto_merge_incoming_wins_overlap() {
        let local = vec![bubble("a", "local", 1)];
        let incoming = vec![bubble("a", "incoming", 1), bubble("b", "new", 2)];
        let (merged, _archive) = auto_merge(&local, &incoming);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|b| b.id == "a" && b.text == "incoming"));
    }
}
