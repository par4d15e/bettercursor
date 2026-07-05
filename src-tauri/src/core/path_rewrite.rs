//! Cross-device project path rewriting (v0.3.6 G2).
//!
//! When applying a remote v4 snapshot, `project_path` from the source
//! machine (e.g. `/Users/...`) must map to a valid local cwd.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::{canonical, config, paths};

/// Map a remote `project_path` to a local cwd for L2/L3 writes.
pub fn rewrite_project_path(remote_path: &str, project_slug: &str) -> String {
    let remote = remote_path.trim();
    if remote.is_empty() {
        return lookup_by_slug(project_slug).unwrap_or_default();
    }

    if Path::new(remote).is_dir() {
        return remote.to_string();
    }

    let prefs = config::load();
    if let Some(mapped) = apply_path_mappings(remote, &prefs.path_mappings) {
        if Path::new(&mapped).is_dir() {
            return mapped;
        }
    }

    if let Some(local) = lookup_by_slug(project_slug) {
        return local;
    }

    remote.to_string()
}

/// Recursively replace `old_prefix` with `new_prefix` in JSON string values.
/// Skips `conversationState` keys (base64 protobuf must not be touched).
pub fn rewrite_paths_in_data(data: &Value, old_prefix: &str, new_prefix: &str) -> Value {
    if old_prefix.is_empty() || old_prefix == new_prefix {
        return data.clone();
    }
    match data {
        Value::String(s) => {
            if s.contains(old_prefix) {
                Value::String(s.replace(old_prefix, new_prefix))
            } else {
                Value::String(s.clone())
            }
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| rewrite_paths_in_data(v, old_prefix, new_prefix))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let child = if k == "conversationState" {
                        v.clone()
                    } else {
                        rewrite_paths_in_data(v, old_prefix, new_prefix)
                    };
                    (k.clone(), child)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn apply_path_mappings(path: &str, mappings: &HashMap<String, String>) -> Option<String> {
    let mut best: Option<(&str, &str)> = None;
    for (from, to) in mappings {
        if path.starts_with(from) {
            let len = from.len();
            if best.map(|(f, _)| len > f.len()).unwrap_or(true) {
                best = Some((from.as_str(), to.as_str()));
            }
        }
    }
    best.map(|(from, to)| {
        let rest = path.strip_prefix(from).unwrap_or(path);
        format!("{}{}", to.trim_end_matches('/'), rest)
    })
}

fn lookup_by_slug(project_slug: &str) -> Option<String> {
    let slug = project_slug.trim();
    if slug.is_empty() || slug == "no-workspace" {
        return None;
    }

    if let Ok(sessions) = canonical::visible_sessions() {
        for s in sessions {
            if s.project_slug == slug && !s.project_path.is_empty() {
                if Path::new(&s.project_path).is_dir() {
                    return Some(s.project_path);
                }
            }
        }
    }

    let projects_dir = paths::cursor_projects_dir().join(slug);
    if projects_dir.is_dir() {
        if let Ok(sessions) = canonical::visible_sessions() {
            for s in sessions {
                if s.project_slug == slug && !s.project_path.is_empty() {
                    return Some(s.project_path);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rewrite_paths_in_data_replaces_nested_strings() {
        let data = json!({
            "workspace": {"path": "/Users/eric/proj/src/main.rs"},
            "conversationState": "/Users/eric/should-not-touch"
        });
        let out = rewrite_paths_in_data(&data, "/Users/eric/proj", "/home/eric/proj");
        assert_eq!(
            out["workspace"]["path"],
            "/home/eric/proj/src/main.rs"
        );
        assert_eq!(
            out["conversationState"],
            "/Users/eric/should-not-touch"
        );
    }

    #[test]
    fn apply_path_mappings_longest_prefix_wins() {
        let mut m = HashMap::new();
        m.insert("/Users/eric".into(), "/home/eric".into());
        m.insert("/Users/eric/workspace".into(), "/home/eric/workspace".into());
        let out = apply_path_mappings("/Users/eric/workspace/foo", &m).unwrap();
        assert_eq!(out, "/home/eric/workspace/foo");
    }

    #[test]
    fn rewrite_project_path_keeps_existing_local_path() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let local = format!("{home}/.bettercursor");
        let _ = std::fs::create_dir_all(&local);
        assert_eq!(rewrite_project_path(&local, "ignored"), local);
    }
}
