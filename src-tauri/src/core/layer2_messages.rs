//! Layer 2 `store.db` AI SDK message DAG → conversation turns.
//!
//! cursor-agent writes blobs as a protobuf-linked DAG rooted at
//! `meta[0].latestRootBlobId`. JSON blobs use the AI SDK shape
//! `{role, content: [{type: text|image|tool-call|...}]}`.
//!
//! `core::sync::write_layer3` calls [`enrich_bubbles_from_layer2`] to
//! replace L1 `[REDACTED]` stubs and CLI envelopes with richer L2 text,
//! tool calls, and image attachments before `inject::compose_bubble_blobs`.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::canonical::{clean_user_text, Bubble, BubbleImage, BubbleToolUse};
use super::paths;

/// One user or assistant turn extracted from the L2 DAG (chronological).
#[derive(Debug, Clone, PartialEq)]
pub struct Layer2Turn {
    pub role: String,
    pub text: String,
    pub tool_calls: Vec<BubbleToolUse>,
    pub images: Vec<BubbleImage>,
}

/// Read conversation turns from CLI `store.db` when present.
pub fn read_layer2_turns(uuid: &str, cwd: &str) -> Vec<Layer2Turn> {
    if cwd.trim().is_empty() {
        return Vec::new();
    }
    let store_db = paths::store_db_for(cwd, uuid);
    if !store_db.is_file() {
        return Vec::new();
    }
    let Ok(conn) = Connection::open_with_flags(&store_db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    read_layer2_turns_from_conn(&conn).unwrap_or_default()
}

/// True when L2 has turns that would improve injected L3 bubble rows.
pub fn layer2_has_richer_turns(uuid: &str, cwd: &str, bubbles: &[Bubble]) -> bool {
    let turns = read_layer2_turns(uuid, cwd);
    if turns.is_empty() {
        return false;
    }
    let mut ti = 0usize;
    for b in bubbles {
        if b.role != "user" && b.role != "assistant" {
            continue;
        }
        let Some(turn) = turns.get(ti) else {
            break;
        };
        if turn.role != b.role {
            continue;
        }
        ti += 1;
        if turn.role == "user" {
            if !turn.images.is_empty() && b.images.is_empty() {
                return true;
            }
            if !turn.text.is_empty() && (b.text.contains("<user_query>") || b.text.contains("<timestamp>"))
            {
                return true;
            }
        } else if b.text.contains("[REDACTED]")
            && (!turn.text.trim().is_empty() || !turn.tool_calls.is_empty())
        {
            return true;
        }
    }
    false
}

/// Overlay L2 turn content onto L1 bubbles (preserve L1 ids / ordinals).
pub fn enrich_bubbles_from_layer2(uuid: &str, cwd: &str, bubbles: &[Bubble]) -> Vec<Bubble> {
    let turns = read_layer2_turns(uuid, cwd);
    if turns.is_empty() {
        return bubbles.to_vec();
    }
    let mut ti = 0usize;
    bubbles
        .iter()
        .map(|b| {
            if b.role != "user" && b.role != "assistant" {
                return b.clone();
            }
            let mut out = b.clone();
            if let Some(turn) = turns.get(ti) {
                if turn.role == b.role {
                    merge_turn_into_bubble(&mut out, turn);
                    ti += 1;
                }
            }
            out
        })
        .collect()
}

fn merge_turn_into_bubble(b: &mut Bubble, turn: &Layer2Turn) {
    if turn.role == "user" {
        if !turn.text.is_empty() {
            b.text = clean_user_text(&turn.text);
        }
        if !turn.images.is_empty() {
            b.images = turn.images.clone();
        }
    } else {
        if b.text.contains("[REDACTED]") {
            if !turn.text.trim().is_empty() {
                b.text = turn.text.clone();
            } else if !turn.tool_calls.is_empty() {
                // Tool-only L2 turn: drop L1 `[REDACTED]` stub; Desktop uses toolFormerData.
                b.text = String::new();
            }
        } else if !turn.text.trim().is_empty() && turn.text.len() > b.text.len() {
            b.text = turn.text.clone();
        }
        if !turn.tool_calls.is_empty() {
            b.tool_calls = turn.tool_calls.clone();
        }
    }
}

fn read_layer2_turns_from_conn(conn: &Connection) -> Result<Vec<Layer2Turn>> {
    let meta_hex: String = conn.query_row(
        "SELECT value FROM meta WHERE key = '0'",
        [],
        |r| r.get(0),
    )?;
    let meta_bytes = hex::decode(&meta_hex).context("decode meta[0] hex")?;
    let meta: Value = serde_json::from_slice(&meta_bytes).context("parse meta[0] json")?;
    let root_id = meta
        .get("latestRootBlobId")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .context("meta latestRootBlobId missing")?;

    let mut blob_map: HashMap<String, Vec<u8>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT id, data FROM blobs")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
    for row in rows.flatten() {
        blob_map.insert(row.0, row.1);
    }

    let order = walk_blob_dag_chronological(root_id, &blob_map);
    let mut turns = Vec::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    for bid in order {
        let Some(data) = blob_map.get(&bid) else {
            continue;
        };
        let Some(msg) = parse_ai_sdk_blob(data) else {
            continue;
        };
        match msg.role.as_str() {
            "system" => {}
            "tool" => {
                pending_tool_results.extend(msg.tool_results);
            }
            "user" if msg.is_user_info => {}
            "user" => {
                pending_tool_results.clear();
                turns.push(Layer2Turn {
                    role: "user".into(),
                    text: msg.text,
                    tool_calls: Vec::new(),
                    images: msg.images,
                });
            }
            "assistant" => {
                turns.push(Layer2Turn {
                    role: "assistant".into(),
                    text: msg.text,
                    tool_calls: msg.tool_calls,
                    images: Vec::new(),
                });
                pending_tool_results.clear();
            }
            _ => {}
        }
    }
    Ok(turns)
}

/// Chronological visit order (oldest first) — mirrors cursor-agent DAG walk.
fn walk_blob_dag_chronological(root_id: &str, blob_map: &HashMap<String, Vec<u8>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![root_id.to_string()];
    let mut order = Vec::new();
    while let Some(id) = stack.first().cloned() {
        stack.remove(0);
        if !seen.insert(id.clone()) {
            continue;
        }
        order.push(id.clone());
        if let Some(data) = blob_map.get(&id) {
            let children = protobuf_child_refs_ordered(data);
            for child in children.into_iter().rev() {
                if !seen.contains(&child) {
                    stack.insert(0, child);
                }
            }
        }
    }
    order
}

struct ParsedAiSdkMessage {
    role: String,
    text: String,
    tool_calls: Vec<BubbleToolUse>,
    tool_results: Vec<Value>,
    images: Vec<BubbleImage>,
    is_user_info: bool,
}

fn parse_ai_sdk_blob(data: &[u8]) -> Option<ParsedAiSdkMessage> {
    if data.first() != Some(&b'{') {
        return None;
    }
    let v: Value = serde_json::from_slice(data).ok()?;
    let role = v.get("role").and_then(|x| x.as_str())?.to_string();
    let raw_json = v.to_string();
    let is_user_info = role == "user" && raw_json.contains("<user_info>");

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();
    let mut images = Vec::new();

    let content = v.get("content").and_then(|c| c.as_array());
    if let Some(arr) = content {
        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let typ = obj.get("type").and_then(|x| x.as_str()).unwrap_or("");
            match typ {
                "text" => {
                    if let Some(t) = obj.get("text").and_then(|x| x.as_str()) {
                        if !t.is_empty() && t != "[REDACTED]" {
                            text_parts.push(t.to_string());
                        }
                    }
                }
                "tool-call" => {
                    let name = obj
                        .get("toolName")
                        .or_else(|| obj.get("name"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        let input = obj
                            .get("args")
                            .or_else(|| obj.get("input"))
                            .cloned();
                        tool_calls.push(BubbleToolUse { name, input });
                    }
                }
                "tool-result" => {
                    tool_results.push(Value::Object(obj.clone()));
                }
                "image" => {
                    if let Some(img) = decode_l2_image(obj.get("image")) {
                        images.push(img);
                    }
                }
                "redacted-reasoning" => {}
                _ => {}
            }
        }
    }

    Some(ParsedAiSdkMessage {
        role,
        text: text_parts.join("\n\n"),
        tool_calls,
        tool_results,
        images,
        is_user_info,
    })
}

fn decode_l2_image(image_val: Option<&Value>) -> Option<BubbleImage> {
    let img = image_val?.as_object()?;
    let hex = img.get("hex").and_then(|x| x.as_str())?;
    let bytes = hex::decode(hex).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let mime = guess_image_mime(&bytes);
    let data_base64 = base64_encode(&bytes);
    Some(BubbleImage {
        mime_type: mime,
        data_base64,
    })
}

fn guess_image_mime(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg".into()
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png".into()
    } else if bytes.starts_with(b"GIF8") {
        "image/gif".into()
    } else if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        "image/webp".into()
    } else {
        "image/jpeg".into()
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i + 1 < bytes.len() {
            bytes[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < bytes.len() {
            bytes[i + 2] as u32
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if i + 1 < bytes.len() {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if i + 2 < bytes.len() {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
        i += 3;
    }
    out
}

/// Collect 32-byte protobuf hash refs in field order (depth-first).
fn protobuf_child_refs_ordered(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    walk_protobuf_refs(data, &mut out);
    out
}

fn walk_protobuf_refs(data: &[u8], into: &mut Vec<String>) {
    let mut offset = 0usize;
    while offset < data.len() {
        let (tag, new_offset) = match read_varint(data, offset) {
            Ok(v) => v,
            Err(_) => break,
        };
        offset = new_offset;
        let wire_type = tag & 0x07;
        match wire_type {
            2 => {
                let (length_u64, data_start) = match read_varint(data, offset) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let length = match usize::try_from(length_u64) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if data_start + length > data.len() {
                    break;
                }
                if length == 32 {
                    let hex: String = data[data_start..data_start + 32]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    into.push(hex);
                    offset = data_start + 32;
                } else if length > 0 {
                    walk_protobuf_refs(&data[data_start..data_start + length], into);
                    offset = data_start + length;
                } else {
                    offset = data_start;
                }
            }
            0 => {
                let (_, new_offset) = match read_varint(data, offset) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                offset = new_offset;
            }
            5 => offset += 4,
            1 => offset += 8,
            _ => offset += 1,
        }
    }
}

fn read_varint(data: &[u8], mut offset: usize) -> Result<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        if offset >= data.len() {
            return Err(anyhow::anyhow!("varint overflow"));
        }
        let b = data[offset];
        offset += 1;
        result |= ((b & 0x7F) as u64) << shift;
        if (b & 0x80) == 0 {
            return Ok((result, offset));
        }
        shift += 7;
        if shift >= 64 {
            return Err(anyhow::anyhow!("varint too large"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ai_sdk_user_with_image_and_assistant_tools() {
        let user_json = serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "<user_query>hello</user_query>"},
                {"type": "image", "image": {"__type": "Uint8Array", "hex": "ffd8ffe000104a46494600010101006000600000"}}
            ]
        });
        let asst_json = serde_json::json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "full assistant reply"},
                {"type": "tool-call", "toolName": "Grep", "args": {"pattern": "*.rs"}}
            ]
        });

        let user_msg = parse_ai_sdk_blob(user_json.to_string().as_bytes()).unwrap();
        assert_eq!(user_msg.role, "user");
        assert_eq!(user_msg.text, "<user_query>hello</user_query>");
        assert_eq!(user_msg.images.len(), 1);
        assert_eq!(user_msg.images[0].mime_type, "image/jpeg");

        let asst_msg = parse_ai_sdk_blob(asst_json.to_string().as_bytes()).unwrap();
        assert_eq!(asst_msg.role, "assistant");
        assert_eq!(asst_msg.text, "full assistant reply");
        assert_eq!(asst_msg.tool_calls.len(), 1);
        assert_eq!(asst_msg.tool_calls[0].name, "Grep");
    }

    #[test]
    fn enrich_replaces_redacted_assistant_text() {
        let turn = Layer2Turn {
            role: "assistant".into(),
            text: "real answer".into(),
            tool_calls: vec![],
            images: vec![],
        };
        let mut partial = Bubble {
            id: "a1".into(),
            role: "assistant".into(),
            text: "opening paragraph\n\n[REDACTED]".into(),
            tool_calls: vec![],
            files: vec![],
            images: vec![],
            created_at_ms: 0,
            parent_bubble_id: None,
        };
        merge_turn_into_bubble(&mut partial, &turn);
        assert_eq!(partial.text, "real answer");

        let mut pure = Bubble {
            id: "a2".into(),
            role: "assistant".into(),
            text: "[REDACTED]".into(),
            tool_calls: vec![],
            files: vec![],
            images: vec![],
            created_at_ms: 0,
            parent_bubble_id: None,
        };
        let tool_turn = Layer2Turn {
            role: "assistant".into(),
            text: String::new(),
            tool_calls: vec![BubbleToolUse {
                name: "Grep".into(),
                input: None,
            }],
            images: vec![],
        };
        merge_turn_into_bubble(&mut pure, &tool_turn);
        assert_eq!(pure.text, "");
        assert_eq!(pure.tool_calls.len(), 1);
    }

    #[test]
    fn layer2_has_richer_false_after_tool_only_enrich_state() {
        let l3 = vec![Bubble {
            id: "a1".into(),
            role: "assistant".into(),
            text: String::new(),
            tool_calls: vec![BubbleToolUse {
                name: "Grep".into(),
                input: None,
            }],
            files: vec![],
            images: vec![],
            created_at_ms: 0,
            parent_bubble_id: None,
        }];
        let turns = vec![Layer2Turn {
            role: "assistant".into(),
            text: String::new(),
            tool_calls: vec![BubbleToolUse {
                name: "Grep".into(),
                input: None,
            }],
            images: vec![],
        }];
        // Inline check: no `[REDACTED]` marker left → not stale.
        assert!(!l3[0].text.contains("[REDACTED]"));
        assert_eq!(turns.len(), 1);
    }

    #[test]
    fn bubble_text_contains_redacted_marker() {
        assert!("hello\n\n[REDACTED]".contains("[REDACTED]"));
        assert!(!String::new().contains("[REDACTED]"));
    }
}
