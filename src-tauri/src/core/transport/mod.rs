//! bettercursor transport — 跨设备 sync 的传输层 (v0.2.6 first cut).
//!
//! 设计取舍 (沿用 SYNC_DESIGN §4.4 但有意偏离 spec):
//! - **同步 trait** (spec 写 async_trait). 项目 Cargo.toml 当前没有 tokio,
//!   业务代码全 std::thread + tauri 内部 runtime. v0.3.0 上 异步 I/O 量
//!   (outbox / 5-way conflict / 3+ transport impl) 再迁. 4 个方法的
//!   trait 同步签名也写得下, 公开 API surface 小, 重制成本低.
//! - **0 个新 Cargo dep**: SSH/rsync 走 `std::process::Command` 调系统
//!   二进制. 进程调用 + stderr 捕获完全够 v0.2.6 用. v0.3.0+ 真要在
//!   Windows native (非 WSL) 跑再引 `russh` / `ssh2`.
//! - "session 端" 概念: 一条 session 在一台机器上的 metadata 表达.
//!   v0.2.6 不搬 bubbles (那是 v0.3.0 codec + unified.db 的活).
//!
//! 模块:
//!   - `snapshot` — 最小 SessionSnapshot 载体 + serde codec
//!   - `ssh`      — SshRsyncTransport (T2) impl, 调系统 ssh + rsync
//!   - `config`   — `~/.bettercursor/transports.json` 读写

use anyhow::Result;

pub mod config;
pub mod snapshot;
pub mod ssh;

pub use config::{PeerConfig, TransportConfigFile};
pub use snapshot::{decode_snapshot, encode_snapshot, SessionSnapshot};
pub use ssh::SshRsyncTransport;

/// 跨设备 sync transport. Push/pull session metadata, 列出远端有什么.
///
/// v0.2.6 first cut: 只有 SSH+rsync (T2) impl. v0.3.0+ 加
/// git (T3) / S3 (T4) / Tailscale (T5) / folder watcher (T1).
///
/// 所有方法的语义:
/// - **idempotent**: caller 可在 transient failure 后重试. push 走
///   tmp + rename, 不会写半个文件. pull 用 `since_ms` 过滤, 不会
///   重复处理同一 epoch 内的元数据.
/// - **`Send + Sync` bound**: 允许存进 AppState Mutex; 同时给 v0.3.0
///   上 async runtime 留余地 (移 async 时只要加 `async` 关键字).
pub trait Transport: Send + Sync {
    /// 推一条 session 的 metadata 到远端. 返回 PushReport (字节数 +
    /// 耗时), 失败时 anyhow::Err 携带 ssh/rsync 的 stderr.
    fn push(&self, snap: &SessionSnapshot) -> Result<PushReport>;

    /// 拉取所有 `last_updated_at_ms >= since_ms` 的远端 snapshot.
    /// 返回顺序: `last_updated_at_ms` 升序.
    ///
    /// v0.2.6 拿到后**不**写 local DB (没 unified.db); 只把数据返回给
    /// 调用方 (调试 / dev console). v0.3.0 unified.db 上后才会落盘.
    fn pull(&self, since_ms: i64) -> Result<Vec<SessionSnapshot>>;

    /// 列出远端所有 session metadata (一次性, 不过滤 since_ms).
    /// 给未来的 "远端 session picker" UI 用 (`<SyncPeersDialog>`).
    fn list_remote(&self) -> Result<Vec<RemoteSessionMeta>>;

    /// 给日志 / UI 用的人类可读 ID, e.g. `"ssh:macbook"`.
    fn endpoint_id(&self) -> &str;
}

/// `Transport::push` 的返回. `uuid` 反映成功推送的 session, `bytes_written`
/// 是 serialised JSON 的字节数 (不含临时文件), `duration_ms` 是 ssh
/// 两次调用 (`mkdir` + heredoc write + `mv`) 的总耗时.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PushReport {
    pub uuid: String,
    pub bytes_written: u64,
    pub duration_ms: u64,
}

/// `Transport::list_remote` 的返回. 是 `SessionSnapshot` 的子集 — 不含
/// `text_preview` 和 `bubble_count` (UI 列远端时不需要, 减少带宽).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteSessionMeta {
    pub uuid: String,
    /// 这条 snapshot 是从哪台机器推到远端的. 跨设备识别用 — 同一
    /// `uuid` 可能存在于多个 peer 上, host 用来在 UI 上分组.
    pub host: String,
    pub last_updated_at_ms: i64,
    pub project_slug: String,
    /// 远端机器上 snapshot 对应的本地 path (e.g. JSONL 绝对路径).
    /// 不一定在本机存在 — 仅作为 metadata 透传.
    pub source_path: String,
}

/// `transport_list_peers` Tauri 命令的返回形状. 是 `PeerConfig` 的
/// serde-friendly 视图 (本身跟 PeerConfig 等价, 但单独一个类型让前端
/// 看到的是"返回这个, 不是配置层"). v0.2.6 first cut 两个 struct 字段
/// 完全一致; v0.3.0 加 UI-only 字段 (e.g. `last_pushed_at`) 时不会破
/// `PeerConfig`.
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
    /// `PeerConfig` → `PeerSummary` 直接透传字段. v0.2.6 first cut
    /// 两个结构等价; 未来 divergence 时这是迁移点.
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

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `PushReport` 字段必须 Serialize 全 (前端 invoke 返回 JSON).
    /// 不允许某字段意外 drop. 这个测试锁住结构形状.
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

    /// `RemoteSessionMeta` 必须 round-trip: 写出去再读回来字段不变.
    /// 因为 list_remote 的结果会进 unified.db (v0.3.0) 或 transient
    /// cache, 必须双向可序列化.
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
        assert_eq!(m.last_updated_at_ms, back.last_updated_at_ms);
        assert_eq!(m.project_slug, back.project_slug);
        assert_eq!(m.source_path, back.source_path);
    }
}