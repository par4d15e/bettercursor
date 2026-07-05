//! v0.2.6 minimal SessionSnapshot — metadata-only 载体.
//!
//! 只承载 8 个字段: uuid / 时间戳 / host / project metadata / 路径 /
//! text preview / bubble 计数. **不**含 bubbles / blobs / 工具调用细节.
//!
//! v0.3.0 §2 codec 会在这个结构上加 fields (bubbles / blob_refs /
//! raw_blobs). 迁移路径: 保留所有现有字段, 新字段用 `#[serde(default)]`,
//! 老 snapshot JSON 仍能 decode, 新字段在 v0.2.6 写的文件里是缺失值.
//!
//! 字符边界: `text_preview` 截断用 `chars().take(280)` (Unicode scalar
//! value 计数), 不会切到 UTF-8 多字节中间.

use serde::{Deserialize, Serialize};

use crate::core::canonical::CanonicalSession;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// Cursor session uuid (32-hex 或 36-char GUID 两种都见过).
    pub uuid: String,
    /// epoch 毫秒. 取 `CanonicalSession::last_updated_at` (已经过
    /// Layer 3 优先级的 last_updated_at 校正, #102).
    pub last_updated_at_ms: i64,
    /// 这条 snapshot 是哪台机器生成的. 跨设备识别用 — 远端存目录会
    /// 用 `<remote_snap_dir>/<host>/<uuid>.json` 命名, 避免同一 uuid
    /// 在两台机器上互相覆盖.
    pub host: String,
    /// Cursor Layer 1/3 的项目 slug (e.g. `Users-eric-workspace-foo`).
    /// `chat-<md5>` fallback 也走这个字段.
    pub project_slug: String,
    /// 项目绝对路径 (e.g. `/home/eric/workspace/enenzuo`). `None`
    /// 时存空串, deserialize 端用 `unwrap_or_default()` 取.
    pub project_path: String,
    /// 这条 session 在源机器上的本地路径 (JSONL 绝对路径优先, 否则
    /// store.db 路径, 最后 state.vscdb 路径). 仅作 metadata 透传,
    /// 不一定在本机存在.
    pub source_path: String,
    /// 第一条 user 消息的纯文本预览, 截 280 字符. 给远端 UI 显示用
    /// ("这条 session 是关于什么的?") — 不含 markdown / 不含工具调用.
    pub text_preview: String,
    /// bubble 计数 (Layer 3 `bubbleCount` 优先, L1/L2 fallback).
    pub bubble_count: u32,
}

impl SessionSnapshot {
    /// 从 `CanonicalSession` 构造. `host` 由调用方注入 (一般是
    /// `hostname` 命令的输出), 不在 `CanonicalSession` 里 — 因为
    /// canonical view 是本机的, host 不属于 "session 本身的属性".
    pub fn from_canonical(c: &CanonicalSession, host: &str) -> Self {
        // 源路径按优先级取 (L3 mac > L2 cli > L3 linux_desktop > 空).
        // 见 SYNC_DESIGN §4.1: `sources.mac.path` 是 Desktop 在 mac 上的
        // 真实存储位置, 优先透传.
        let source_path = c
            .sources
            .mac
            .as_ref()
            .or(c.sources.linux_desktop.as_ref())
            .or(c.sources.linux_cli.as_ref())
            .map(|s| s.path.clone())
            .unwrap_or_default();

        Self {
            uuid: c.uuid.clone(),
            last_updated_at_ms: c.last_updated_at,
            host: host.to_string(),
            project_slug: c.project_slug.clone(),
            project_path: c.project_path.clone(),
            source_path,
            text_preview: truncate_chars(&c.first_user_message_preview, 280),
            bubble_count: c.bubble_count,
        }
    }
}

/// 按 Unicode scalar value 截断到 `max` 字符. 不会切到 UTF-8 多字节
/// 中间 — 用 `chars().take()` 天然安全.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 序列化为紧凑 JSON 字符串. `serde_json::to_string` 已经默认紧凑
/// (无 indent), 这里抽出来是为了让 transport impl 用一个明确名字
/// 而不是裸 `serde_json::to_string` (grep 时好找).
pub fn encode_snapshot(s: &SessionSnapshot) -> Result<String, serde_json::Error> {
    serde_json::to_string(s)
}

/// 反序列化. 任何字段缺失都会直接 Err — v0.2.6 不允许降级 decode
/// (因为字段就 8 个, 老 snapshot 是同版本同 schema 写的).
pub fn decode_snapshot(json: &str) -> Result<SessionSnapshot, serde_json::Error> {
    serde_json::from_str(json)
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::canonical::CanonicalSession;

    fn minimal_canonical() -> CanonicalSession {
        CanonicalSession {
            uuid: "uuid-1234".into(),
            project_slug: "Users-eric-workspace-enenzuo".into(),
            project_path: "/Users/eric/workspace/enenzuo".into(),
            chat_root: String::new(),
            name: "session".into(),
            last_updated_at: 1_700_000_000_000,
            bubble_count: 7,
            is_empty_draft: false,
            is_broken: false,
            broken_reason: None,
            sources: Default::default(),
            first_user_message_preview: String::new(),
            files_referenced: vec![],
            indexable_text: String::new(),
            layer_3_present: false,
            layer_3_needs_refresh: false,
            layer_2_needs_refresh: false,
            created_endpoint: None,
            created_at_ms: None,
            composer_data: None,
            composer_id: None,
            is_subagent: false,
            subagent_info: None,
        }
    }

    /// 字段映射: 8 个字段逐个对照. 这是 `from_canonical` 的主合约.
    #[test]
    fn from_canonical_maps_all_fields() {
        let mut c = minimal_canonical();
        c.first_user_message_preview = "hello world".into();
        c.sources.mac = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 1_700_000_000_000,
            layer: "3".into(),
            path: "/Users/eric/.cursor/projects/foo/transcript.jsonl".into(),
        });
        let snap = SessionSnapshot::from_canonical(&c, "macbook-pro");
        assert_eq!(snap.uuid, "uuid-1234");
        assert_eq!(snap.last_updated_at_ms, 1_700_000_000_000);
        assert_eq!(snap.host, "macbook-pro");
        assert_eq!(snap.project_slug, "Users-eric-workspace-enenzuo");
        assert_eq!(snap.project_path, "/Users/eric/workspace/enenzuo");
        assert_eq!(
            snap.source_path,
            "/Users/eric/.cursor/projects/foo/transcript.jsonl"
        );
        assert_eq!(snap.text_preview, "hello world");
        assert_eq!(snap.bubble_count, 7);
    }

    /// `source_path` 优先级: mac > linux_desktop > linux_cli > 空.
    /// SYNC_DESIGN §4.1 mac 是 Desktop 在 macOS 上的真实路径, 最优先.
    #[test]
    fn source_path_priority_order() {
        let mut c = minimal_canonical();
        c.sources.linux_cli = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 0,
            layer: "2".into(),
            path: "/cli/store.db".into(),
        });
        c.sources.linux_desktop = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 0,
            layer: "3".into(),
            path: "/desktop/state.vscdb".into(),
        });
        c.sources.mac = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 0,
            layer: "3".into(),
            path: "/mac/state.vscdb".into(),
        });
        let snap = SessionSnapshot::from_canonical(&c, "h");
        assert_eq!(snap.source_path, "/mac/state.vscdb", "mac wins");

        let mut c2 = minimal_canonical();
        c2.sources.linux_cli = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 0,
            layer: "2".into(),
            path: "/cli/store.db".into(),
        });
        c2.sources.linux_desktop = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 0,
            layer: "3".into(),
            path: "/desktop/state.vscdb".into(),
        });
        let snap2 = SessionSnapshot::from_canonical(&c2, "h");
        assert_eq!(snap2.source_path, "/desktop/state.vscdb", "linux_desktop 2nd");

        let mut c3 = minimal_canonical();
        c3.sources.linux_cli = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 0,
            layer: "2".into(),
            path: "/cli/store.db".into(),
        });
        let snap3 = SessionSnapshot::from_canonical(&c3, "h");
        assert_eq!(snap3.source_path, "/cli/store.db", "linux_cli last fallback");

        let c4 = minimal_canonical();
        let snap4 = SessionSnapshot::from_canonical(&c4, "h");
        assert_eq!(snap4.source_path, "");
    }

    /// encode → decode round-trip 必须保持所有字段相等. 这是 transport
    /// push/pull 的基础保证 — 写出去的 JSON 必须能被自己读回来.
    #[test]
    fn encode_decode_round_trip() {
        let mut c = minimal_canonical();
        c.first_user_message_preview = "hello 你好 🎉".into();
        c.sources.linux_cli = Some(crate::core::canonical::SourceInfo {
            last_seen_at: 1,
            layer: "2".into(),
            path: "/foo.db".into(),
        });
        let snap = SessionSnapshot::from_canonical(&c, "test-host");
        let json = encode_snapshot(&snap).expect("encode");
        let back = decode_snapshot(&json).expect("decode");
        assert_eq!(snap, back, "round-trip must preserve equality");
        // 顺便验证 JSON 里中文 / emoji 没被 escape 截断
        assert!(json.contains("hello 你好 🎉"), "UTF-8 preserved in JSON: {json}");
    }

    /// text_preview 截断到 280 字符 (Unicode scalar value, 不是字节).
    /// 这是 bug #XX (假想) 的预防 — 用 `chars().take(280)` 安全.
    #[test]
    fn text_preview_truncates_to_280_chars() {
        let mut c = minimal_canonical();
        c.first_user_message_preview = "中".repeat(500); // 500 个汉字 = 1500 字节
        let snap = SessionSnapshot::from_canonical(&c, "h");
        assert_eq!(snap.text_preview.chars().count(), 280);
        // 截断位置必须在 char boundary, 不能切到半个 UTF-8 序列
        assert!(snap.text_preview.is_char_boundary(snap.text_preview.len()));
        assert_eq!(snap.text_preview, "中".repeat(280));
    }

    /// text_preview 短字符串不动 (< 280).
    #[test]
    fn text_preview_passes_through_short_strings() {
        let mut c = minimal_canonical();
        c.first_user_message_preview = "short".into();
        let snap = SessionSnapshot::from_canonical(&c, "h");
        assert_eq!(snap.text_preview, "short");
    }

    /// 空 preview (Layer 3 注入的 session 没 user message) 不崩.
    #[test]
    fn text_preview_handles_empty() {
        let c = minimal_canonical();
        let snap = SessionSnapshot::from_canonical(&c, "h");
        assert_eq!(snap.text_preview, "");
    }

    /// Decode 失败: 缺字段必须 Err, 不允许静默默认值. v0.2.6
    /// 不做降级 decode, 因为 schema 简单 + 跨版本要显式.
    #[test]
    fn decode_missing_field_errors() {
        let bad = r#"{"uuid":"a","last_updated_at_ms":1}"#; // 缺 host/project_slug 等
        let r = decode_snapshot(bad);
        assert!(r.is_err(), "must reject partial snapshot");
    }
}