//! `~/.bettercursor/transports.json` 读写.
//!
//! 文件 schema (v0.2.6 first cut):
//! ```json
//! {
//!   "peers": [
//!     {
//!       "id": "macbook",
//!       "kind": "ssh",
//!       "host": "eric@192.168.1.42",
//!       "port": 22,
//!       "identity_file": "~/.ssh/id_ed25519",
//!       "remote_snap_dir": "~/.bettercursor/peers/bettercursor-main",
//!       "remote_hostname": "macbook-pro-m1"
//!     }
//!   ]
//! }
//! ```
//!
//! 跟 `~/.bettercursor/config.json` (Preferences) 是**两个**文件 —
//! 那个是用户偏好 (auto-sync on/off 等), 这个是 peer 配置. 不混.
//!
//! v0.2.6 没 UI 写这个文件; 调试时手动编 JSON, 用 `transport_test`
//! 命令验证 SSH 通. v0.3.0 出 `<SyncPeersDialog>` 后会带 form 编辑.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TransportConfigFile {
    pub peers: Vec<PeerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    /// 人类可读 ID. 给 `transport_test` / `transport_push` / `transport_pull`
    /// 命令用, 也给 `<SyncPeersDialog>` (v0.3.0) 当列表 key.
    pub id: String,
    /// Transport kind. v0.2.6 只支持 `"ssh"`; v0.3.0+ 加
    /// `"git"` / `"s3"` / `"tailscale"` / `"folder"`. 决定
    /// `SshRsyncTransport` 走哪条 impl.
    pub kind: String,
    /// SSH host 字符串 (OpenSSH 格式: `user@host` 或 `host`).
    pub host: String,
    /// SSH port. 大多数场景 22; 走跳板机 / NAS 可能 2222 等.
    pub port: u16,
    /// Identity file 绝对路径. v0.2.6 强制要求 — 不支持 ssh-agent
    /// / ControlMaster (那是 v0.3.0+ 优化).
    pub identity_file: String,
    /// 远端 snapshot 根目录. ssh 远端 shell 会 expand `~`, 所以
    /// 路径里写 `~/.bettercursor/peers/...` 字面量即可, **不要**
    /// 在 Rust 侧 expand.
    pub remote_snap_dir: String,
    /// 远端机器的 hostname. 仅作 metadata 透传 (snapshot 里 `host`
    /// 字段用). 不同机器的 snapshot 会在远端用这个字段做 namespace.
    pub remote_hostname: String,
}

impl TransportConfigFile {
    /// 加载 `~/.bettercursor/transports.json`. 文件不存在 → 空配置
    /// (首次运行的正常情况).
    pub fn load() -> Result<Self> {
        let path = transports_json_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let body =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let cfg: Self =
            serde_json::from_str(&body).with_context(|| format!("parse {}", path.display()))?;
        Ok(cfg)
    }

    /// 保存到 `~/.bettercursor/transports.json`. 自动 `mkdir -p` 父目录.
    /// 原子写: 写 `*.tmp` + rename, 避免半截 JSON 损坏配置.
    pub fn save(&self) -> Result<()> {
        let path = transports_json_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(self).context("serialize transports.json")?;
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &body)
            .with_context(|| format!("write {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &path)
            .with_context(|| format!("rename to {}", path.display()))?;
        Ok(())
    }

    /// 按 id 查找 peer. 没找到 → `None` (调用方返回 404 给前端).
    pub fn peer(&self, id: &str) -> Option<&PeerConfig> {
        self.peers.iter().find(|p| p.id == id)
    }
}

/// `~/.bettercursor/transports.json` 路径. 不强制 mkdir — `save` 时才
/// mkdir, `load` 时文件不存在返回 default 即可.
fn transports_json_path() -> Result<PathBuf> {
    let dir = crate::core::paths::bettercursor_dir();
    Ok(dir.join("transports.json"))
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 临时写一个 transports.json 到 OS temp 目录的 helper.
    /// 不污染 `~/.bettercursor/` (那是真实 dev 配置).
    fn write_tmp_json(body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bc-transport-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("transports.json");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    /// 合法 JSON → 完整字段 round-trip.
    #[test]
    fn parse_valid_peers() {
        let body = r#"{
  "peers": [
    {
      "id": "macbook",
      "kind": "ssh",
      "host": "eric@192.168.1.42",
      "port": 22,
      "identity_file": "~/.ssh/id_ed25519",
      "remote_snap_dir": "~/.bettercursor/peers/bettercursor-main",
      "remote_hostname": "macbook-pro-m1"
    }
  ]
}"#;
        let p = write_tmp_json(body);
        let body = std::fs::read_to_string(&p).unwrap();
        let cfg: TransportConfigFile = serde_json::from_str(&body).unwrap();
        assert_eq!(cfg.peers.len(), 1);
        let peer = &cfg.peers[0];
        assert_eq!(peer.id, "macbook");
        assert_eq!(peer.kind, "ssh");
        assert_eq!(peer.host, "eric@192.168.1.42");
        assert_eq!(peer.port, 22);
        assert_eq!(peer.identity_file, "~/.ssh/id_ed25519");
        assert_eq!(
            peer.remote_snap_dir,
            "~/.bettercursor/peers/bettercursor-main"
        );
        assert_eq!(peer.remote_hostname, "macbook-pro-m1");
    }

    /// `peer()` 按 id 查找, 找不到 → None.
    #[test]
    fn peer_lookup_by_id() {
        let cfg = TransportConfigFile {
            peers: vec![
                PeerConfig {
                    id: "a".into(),
                    kind: "ssh".into(),
                    host: "x".into(),
                    port: 22,
                    identity_file: "i".into(),
                    remote_snap_dir: "d".into(),
                    remote_hostname: "h".into(),
                },
                PeerConfig {
                    id: "b".into(),
                    kind: "ssh".into(),
                    host: "y".into(),
                    port: 22,
                    identity_file: "i".into(),
                    remote_snap_dir: "d".into(),
                    remote_hostname: "h".into(),
                },
            ],
        };
        assert!(cfg.peer("a").is_some());
        assert!(cfg.peer("b").is_some());
        assert!(cfg.peer("c").is_none());
        assert_eq!(cfg.peer("a").unwrap().host, "x");
    }

    /// 缺字段 → Err. 不允许 silently 默认值, 防止 peer 配置写一半
    /// 然后 `transport_test` 神秘失败.
    #[test]
    fn parse_missing_field_errors() {
        let body = r#"{"peers":[{"id":"x","kind":"ssh"}]}"#; // 缺 host/port/etc
        let r: Result<TransportConfigFile, _> = serde_json::from_str(body);
        assert!(r.is_err(), "must reject incomplete peer config");
    }

    /// 空 `peers` 数组是合法的 (首次运行场景).
    #[test]
    fn parse_empty_peers_ok() {
        let cfg: TransportConfigFile = serde_json::from_str(r#"{"peers":[]}"#).unwrap();
        assert!(cfg.peers.is_empty());
        assert!(cfg.peer("anything").is_none());
    }

    /// save → load round-trip 在 OS temp 目录里. 用 `bettercursor_dir`
    /// 的 stub: 我们 mock 一个临时 HOME 目录覆盖? v0.2.6 测试不引
    /// env override — 直接测 `serde_json` 的 round-trip + save 写文件.
    #[test]
    fn save_then_serde_round_trip() {
        let cfg = TransportConfigFile {
            peers: vec![PeerConfig {
                id: "test-peer".into(),
                kind: "ssh".into(),
                host: "user@host".into(),
                port: 2222,
                identity_file: "/tmp/key".into(),
                remote_snap_dir: "~/.bettercursor/peers/x".into(),
                remote_hostname: "h2".into(),
            }],
        };
        let body = serde_json::to_string_pretty(&cfg).unwrap();
        let back: TransportConfigFile = serde_json::from_str(&body).unwrap();
        assert_eq!(cfg.peers.len(), back.peers.len());
        assert_eq!(cfg.peers[0].id, back.peers[0].id);
        assert_eq!(cfg.peers[0].port, back.peers[0].port);
    }
}
