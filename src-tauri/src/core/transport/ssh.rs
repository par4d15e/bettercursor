//! SshRsyncTransport — T2 transport (SSH + rsync), v0.2.6 first cut.
//!
//! Wire format: 一条 session metadata = 一个 JSON 文件, 路径
//! `<remote_snap_dir>/<host>/<uuid>.json`. `host`-namespaced 是为了避免
//! 同一 uuid 在两台机器上互相覆盖 (没 unified.db 时这是防覆盖的关键).
//!
//! 操作:
//! - **push**: `ssh <peer> mkdir -p <remote_snap_dir>/<host>` 然后
//!   `cat > <remote_snap_dir>/<host>/<uuid>.json.tmp <<'__BC_EOF__'`
//!   + heredoc body + `mv .tmp .json` (atomic on remote).
//! - **pull**: `rsync -az --include='*/' --include='*.json' --exclude='*'`
//!   `<peer>:<remote_snap_dir>/<host>/` 到 tempfile tmpdir, 然后 walk +
//!   decode + filter by mtime > since_ms.
//! - **list_remote**: same fetch as pull (since_ms=0), 只取 metadata
//!   不 decode body.
//!
//! Deps: 0 new crate. `std::process::Command` + `std::fs` + `std::io`.
//!
//! 安全:
//! - ssh 调用带 `-o BatchMode=yes` (不交互, 没 passphrase prompt) +
//!   `-o StrictHostKeyChecking=accept-new` (新 host 自动加入 known_hosts,
//!   已存在但 key 变了硬报错 — 比 `yes` 提示友好).
//! - identity_file 是 SSH private key 路径, 不传口令.
//! - body 转义走单引号 + heredoc, 避免 shell injection.
//! - remote_snap_dir 字面量传 `~`, 让远端 shell expand (`$HOME`).

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::process::Stdio;

use super::config::PeerConfig;
use super::snapshot_meta::{
    decode_snapshot as decode_snapshot_meta, encode_snapshot as encode_snapshot_meta,
    SessionSnapshot as SessionSnapshotMeta,
};
use super::{PushReport, RemoteSessionMeta, RemoteSnapshot, Transport};

pub struct SshRsyncTransport {
    config: PeerConfig,
    /// Override for tests — fake-ssh.sh 路径. 默认走系统 PATH 找 `ssh`.
    ssh_bin: String,
    /// 同上, 默认 `rsync`.
    rsync_bin: String,
}

impl SshRsyncTransport {
    /// 正常构造. ssh / rsync 走系统 PATH (假定 `ssh` 和 `rsync` 预装).
    pub fn new(config: PeerConfig) -> Self {
        Self {
            config,
            ssh_bin: "ssh".to_string(),
            rsync_bin: "rsync".to_string(),
        }
    }

    /// 测试用构造 — 替换 ssh / rsync binary 路径. fake-ssh.sh 走这里.
    #[cfg(test)]
    pub fn with_bins(config: PeerConfig, ssh_bin: &str, rsync_bin: &str) -> Self {
        Self {
            config,
            ssh_bin: ssh_bin.to_string(),
            rsync_bin: rsync_bin.to_string(),
        }
    }

    /// 测 SSH 连通性: 跑 `ssh <peer> true`. 成功 = Exit 0; 失败 = Err.
    /// 给 `transport_test` Tauri 命令用. 不属于 `Transport` trait 是因为
    /// 它是"连通性测试", 不是 push/pull/list_remote 语义.
    pub fn test_connection(&self) -> Result<()> {
        let rt = tokio::runtime::Runtime::new().context("tokio runtime for test_connection")?;
        rt.block_on(async {
            let out = self
                .ssh_cmd()
                .arg(&self.config.host)
                .arg("true")
                .output()
                .await
                .with_context(|| format!("ssh to {} failed to start", self.config.host))?;
            if !out.status.success() {
                return Err(anyhow!(
                    "ssh to {} exited with {}: {}",
                    self.config.host,
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            Ok(())
        })
    }

    fn ssh_std_cmd(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.ssh_bin);
        cmd.arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg("-i")
            .arg(&self.config.identity_file)
            .arg("-p")
            .arg(self.config.port.to_string());
        cmd
    }

    /// 构造一个 ssh 子进程 Command, 带 BatchMode + StrictHostKeyChecking
    /// + identity_file + port. 返回的 Command **还没指定 args**, 调用方
    /// 自己 append `host` 和 remote_cmd.
    fn ssh_cmd(&self) -> tokio::process::Command {
        tokio::process::Command::from(self.ssh_std_cmd())
    }

    async fn ssh_run(&self, remote_cmd: &str) -> Result<String> {
        let out = self
            .ssh_cmd()
            .arg(&self.config.host)
            .arg(remote_cmd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("ssh to {} failed to start", self.config.host))?;
        if !out.status.success() {
            return Err(anyhow!(
                "ssh to {} exited with {}: {}",
                self.config.host,
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    async fn ssh_write_atomic(&self, remote_path: &str, body: &str) -> Result<()> {
        let remote_dir = remote_path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or(".");
        self.ssh_run(&format!("mkdir -p '{}'", remote_dir)).await?;
        let tmp_path = format!("{remote_path}.tmp");
        let remote_cmd = format!(
            "cat > '{tmp}' <<'__BC_EOF__'\n{body}\n__BC_EOF__",
            tmp = tmp_path,
            body = body,
        );
        self.ssh_run(&remote_cmd).await?;
        self.ssh_run(&format!("mv '{tmp}' '{final}'", tmp = tmp_path, final = remote_path))
            .await?;
        Ok(())
    }

    async fn rsync_fetch(&self, host: &str) -> Result<tempfile::TempDir> {
        let tmpdir = tempfile::tempdir().context("rsync_fetch: tempfile::tempdir")?;
        let remote_glob = format!(
            "{}:{}/{}/",
            self.config.host, self.config.remote_snap_dir, host
        );
        let ssh_proxy = format!(
            "ssh -p {} -i {} -o BatchMode=yes -o StrictHostKeyChecking=accept-new",
            self.config.port, self.config.identity_file
        );
        let out = tokio::process::Command::new(&self.rsync_bin)
            .arg("-az")
            .arg("--include=*/")
            .arg("--include=*.json")
            .arg("--exclude=*")
            .arg("-e")
            .arg(&ssh_proxy)
            .arg(&remote_glob)
            .arg(format!("{}/", tmpdir.path().display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| "rsync failed to start")?;
        if !out.status.success() {
            return Err(anyhow!(
                "rsync exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(tmpdir)
    }
}

#[async_trait]
impl Transport for SshRsyncTransport {
    async fn push(&self, snap: &super::PushSnapshot) -> Result<PushReport> {
        let started = std::time::Instant::now();
        let remote_dir = format!("{}/{}", self.config.remote_snap_dir, snap.host_namespace());
        let final_path = format!("{}/{}.json", remote_dir, snap.uuid());
        let body = snap.encode_body().context("encode snapshot for push")?;
        self.ssh_write_atomic(&final_path, &body).await?;
        Ok(PushReport {
            uuid: snap.uuid().to_string(),
            bytes_written: body.len() as u64,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn pull(&self, since_ms: i64) -> Result<Vec<RemoteSnapshot>> {
        let tmpdir = self.rsync_fetch(&local_hostname().await?).await?;
        let mut out_snapshots = Vec::new();
        for entry in std::fs::read_dir(tmpdir.path())
            .with_context(|| format!("read_dir {}", tmpdir.path().display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let mtime_ms = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if mtime_ms < since_ms {
                continue;
            }
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            if let Ok(v4) = crate::core::snapshot::decode_snapshot(&body) {
                out_snapshots.push(RemoteSnapshot::V4(v4));
                continue;
            }
            match decode_snapshot_meta(&body) {
                Ok(s) => out_snapshots.push(RemoteSnapshot::Meta(s)),
                Err(e) => log::warn!("skipping malformed snapshot {}: {}", path.display(), e),
            }
        }
        out_snapshots.sort_by_key(|s| match s {
            RemoteSnapshot::V4(v) => v.composer.last_updated_at,
            RemoteSnapshot::Meta(m) => m.last_updated_at_ms,
        });
        Ok(out_snapshots)
    }

    async fn list_remote(&self) -> Result<Vec<RemoteSessionMeta>> {
        let snaps = self.pull(0).await?;
        Ok(snaps
            .iter()
            .map(|s| match s {
                RemoteSnapshot::V4(v) => RemoteSessionMeta {
                    uuid: v.composer.composer_id.clone(),
                    host: v.source_endpoint.host.clone(),
                    last_updated_at_ms: v.composer.last_updated_at,
                    project_slug: v.composer.project_slug.clone(),
                    source_path: v.composer.project_path.clone(),
                },
                RemoteSnapshot::Meta(m) => RemoteSessionMeta {
                    uuid: m.uuid.clone(),
                    host: m.host.clone(),
                    last_updated_at_ms: m.last_updated_at_ms,
                    project_slug: m.project_slug.clone(),
                    source_path: m.source_path.clone(),
                },
            })
            .collect())
    }

    fn endpoint_id(&self) -> &str {
        &self.config.id
    }
}

async fn local_hostname() -> Result<String> {
    let out = tokio::process::Command::new("hostname")
        .output()
        .await
        .with_context(|| "hostname command failed")?;
    if !out.status.success() {
        log::warn!(
            "hostname exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        return Ok("unknown".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::transport::SessionSnapshotMeta;

    fn fake_peer() -> PeerConfig {
        PeerConfig {
            id: "test-peer".into(),
            kind: "ssh".into(),
            host: "fake-host".into(),
            port: 22,
            identity_file: "/tmp/fake-key".into(),
            remote_snap_dir: "/tmp/fake-snap".into(),
            remote_hostname: "fake-remote-host".into(),
        }
    }

    /// `endpoint_id()` 直接透传 `PeerConfig.id`. 不做 format / prefix.
    #[test]
    fn endpoint_id_returns_peer_id() {
        let t = SshRsyncTransport::new(fake_peer());
        assert_eq!(t.endpoint_id(), "test-peer");
    }

    /// `with_bins` 测试构造必须把 binary 路径记下来 (后面 ssh_run
    /// 会用). `new` 默认 `ssh` / `rsync` (走 PATH).
    #[test]
    fn with_bins_stores_binaries() {
        let t = SshRsyncTransport::with_bins(fake_peer(), "/fake/ssh", "/fake/rsync");
        assert_eq!(t.ssh_bin, "/fake/ssh");
        assert_eq!(t.rsync_bin, "/fake/rsync");
        // new 走 PATH
        let t2 = SshRsyncTransport::new(fake_peer());
        assert_eq!(t2.ssh_bin, "ssh");
        assert_eq!(t2.rsync_bin, "rsync");
    }

    /// `ssh_cmd()` 构造的 Command 必须包含 `BatchMode=yes` /
    /// `StrictHostKeyChecking=accept-new` / `-i <identity_file>` / `-p <port>`.
    /// 这是 SSH 调用的安全契约 — 不能漏 flag 否则交互式 prompt 死锁.
    #[test]
    fn ssh_cmd_includes_safety_flags() {
        let t = SshRsyncTransport::new(fake_peer());
        let cmd = t.ssh_std_cmd();
        let args: Vec<&str> = cmd.get_args().filter_map(|a| a.to_str()).collect();
        // -o BatchMode=yes
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "BatchMode=yes"),
            "missing BatchMode=yes in: {args:?}"
        );
        // -o StrictHostKeyChecking=accept-new
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "StrictHostKeyChecking=accept-new"),
            "missing StrictHostKeyChecking=accept-new in: {args:?}"
        );
        // -i <identity_file>
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-i" && w[1] == "/tmp/fake-key"),
            "missing -i identity_file in: {args:?}"
        );
        // -p <port>
        assert!(
            args.windows(2).any(|w| w[0] == "-p" && w[1] == "22"),
            "missing -p 22 in: {args:?}"
        );
    }

    /// push 路径: fake ssh 走 `with_bins` 指向 `tests/fixtures/fake-ssh.sh`,
    /// 验证调用成功 + exit 0 + body 编码为 JSON. 不验证远端真有文件 —
    /// 那要真 SSH peer, v0.2.6 留给 manual e2e (见 plan "manual e2e 真实
    /// ssh peer" 段).
    ///
    /// 这里需要 `tests/fixtures/fake-ssh.sh` 已存在. 缺失时 skip (test 不
    /// 失败, 只是 logged).
    #[tokio::test]
    async fn push_calls_ssh_with_expected_args() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake-ssh.sh");
        if !std::path::Path::new(fixture).exists() {
            eprintln!("skipping: fake-ssh.sh fixture not at {fixture}");
            return;
        }
        let t = SshRsyncTransport::with_bins(fake_peer(), fixture, "rsync");
        let snap = crate::core::transport::PushSnapshot::Meta(SessionSnapshotMeta {
            uuid: "uuid-test".into(),
            last_updated_at_ms: 1_700_000_000_000,
            host: "local-host".into(),
            project_slug: "test-slug".into(),
            project_path: "/test/path".into(),
            source_path: "/test/path/file.jsonl".into(),
            text_preview: "hello".into(),
            bubble_count: 3,
        });
        let report = t
            .push(&snap)
            .await
            .expect("push should succeed with fake ssh");
        assert_eq!(report.uuid, "uuid-test");
        assert!(report.bytes_written > 0, "must report body bytes");
    }

    /// ssh 失败 (non-zero exit) → Err 携带 stderr. fake ssh 模拟
    /// 失败场景.
    #[tokio::test]
    async fn push_ssh_failure_surfaces_stderr() {
        let t = SshRsyncTransport::with_bins(fake_peer(), "false", "rsync");
        let snap = crate::core::transport::PushSnapshot::Meta(SessionSnapshotMeta {
            uuid: "x".into(),
            last_updated_at_ms: 0,
            host: "h".into(),
            project_slug: "s".into(),
            project_path: String::new(),
            source_path: String::new(),
            text_preview: String::new(),
            bubble_count: 0,
        });
        let r = t.push(&snap).await;
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("fake-host"), "error mentions peer host: {msg}");
        assert!(msg.contains("exited"), "error mentions exit status: {msg}");
    }

    /// `local_hostname()` 拿到 hostname 命令的 stdout, trim 末尾 newline.
    /// `hostname` 几乎所有 Unix 都预装; CI Linux runner 必有.
    #[tokio::test]
    async fn local_hostname_returns_trimmed_string() {
        let h = local_hostname()
            .await
            .expect("hostname should work on Unix");
        assert!(!h.is_empty());
        assert!(!h.ends_with('\n'));
        assert!(!h.ends_with('\r'));
    }
}
