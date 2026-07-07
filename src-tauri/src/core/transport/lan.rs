//! LAN TCP Transport (T2a) — 配对后局域网直连 push/pull v4 snapshot.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::trusted_peers::TrustedPeer;
use super::{PushReport, PushSnapshot, RemoteSessionMeta, RemoteSnapshot, Transport};

const PROTO: &str = "BC/1";

/// 待配对：本机展示 6 位码，对端用 `PAIR` 命令接入。
#[derive(Debug, Clone)]
pub struct PendingPairing {
    pub code: String,
    pub secret: String,
    pub created_at_ms: i64,
}

static PENDING_PAIRING: Mutex<Option<PendingPairing>> = Mutex::new(None);
static LAN_SERVER_PORT: Mutex<Option<u16>> = Mutex::new(None);

/// 作为配对客户端：连接远端并提交配对码。
pub fn pairing_join_client(
    host: &str,
    port: u16,
    code: &str,
    device_id: &str,
    device_name: &str,
) -> Result<TrustedPeer> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("parse {host}:{port}"))?;
    let device_id = device_id.to_string();
    let device_name = device_name.to_string();
    let code = code.to_string();
    let lan_addr = addr.to_string();
    std::thread::spawn(move || -> Result<TrustedPeer> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            tokio::task::spawn_blocking(move || {
                let pair = pair_v2_with_fallback(&addr, &code, &device_id, &device_name)?;
                let peer = TrustedPeer {
                    id: pair.device_id,
                    device_name: pair.device_name,
                    lan_addr,
                    pairing_secret: pair.secret,
                    trusted_at_ms: chrono::Utc::now().timestamp_millis(),
                };
                let mut peers = super::trusted_peers::TrustedPeersFile::load()?;
                peers.upsert(peer.clone());
                peers.save()?;
                Ok(peer)
            })
            .await?
        })
    })
    .join()
    .map_err(|_| anyhow!("pairing thread panicked"))?
}

pub fn snapshots_incoming_dir() -> std::path::PathBuf {
    crate::core::paths::bettercursor_dir().join("snapshots_incoming")
}

/// 生成 6 位配对码 + secret，启动 LAN TCP 服务（若尚未启动）。
pub fn start_pairing_mode() -> Result<PendingPairing> {
    let port = ensure_lan_server()?;
    let code: String = (0..6).map(|_| format!("{}", rand_digit())).collect();
    let secret = hex::encode(rand_bytes(16));
    let pending = PendingPairing {
        code: code.clone(),
        secret: secret.clone(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    *PENDING_PAIRING.lock().unwrap() = Some(pending.clone());
    log::info!("LAN pairing mode on port {port}, code={code}");
    Ok(pending)
}

pub fn lan_listen_port() -> Option<u16> {
    *LAN_SERVER_PORT.lock().unwrap()
}

/// 供 sync_loop / mDNS 使用：确保 LAN TCP 服务已监听。
pub fn ensure_lan_server_public() -> Result<u16> {
    ensure_lan_server()
}

fn ensure_lan_server() -> Result<u16> {
    if let Some(p) = *LAN_SERVER_PORT.lock().unwrap() {
        return Ok(p);
    }
    let listener = TcpListener::bind("0.0.0.0:0").context("bind LAN TCP")?;
    let port = listener.local_addr()?.port();
    *LAN_SERVER_PORT.lock().unwrap() = Some(port);
    std::thread::Builder::new()
        .name("bettercursor-lan-tcp".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = handle_connection(stream);
            }
        })
        .context("spawn lan server")?;
    Ok(port)
}

fn handle_connection(mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut line = String::new();
    read_line(&mut stream, &mut line)?;
    if line != PROTO {
        return write_err(&mut stream, "bad protocol");
    }
    let mut cmd_line = String::new();
    read_line(&mut stream, &mut cmd_line)?;
    let parts: Vec<&str> = cmd_line.split_whitespace().collect();
    match parts.first().copied() {
        Some("PAIR2") => handle_pair_v2(&mut stream, &parts[1..]),
        Some("PAIR") => handle_pair(&mut stream, &parts[1..]),
        Some("PUSH") => handle_push(&mut stream, &parts[1..]),
        Some("PULL") => handle_pull(&mut stream, &parts[1..]),
        _ => write_err(&mut stream, "unknown command"),
    }
}

fn handle_pair(stream: &mut TcpStream, args: &[&str]) -> Result<()> {
    if args.len() < 2 {
        return write_err(stream, "PAIR needs code and device_name");
    }
    let code = args[0];
    let device_name = args[1..].join(" ");
    let pending = PENDING_PAIRING.lock().unwrap().clone();
    let Some(p) = pending else {
        return write_err(stream, "pairing not active");
    };
    if p.code != code {
        return write_err(stream, "invalid code");
    }
    let peer_addr = stream.peer_addr()?.to_string();
    let mut peers = super::trusted_peers::TrustedPeersFile::load()?;
    peers.upsert(TrustedPeer {
        id: hex::encode(rand_bytes(8)),
        device_name: device_name.clone(),
        lan_addr: peer_addr,
        pairing_secret: p.secret.clone(),
        trusted_at_ms: chrono::Utc::now().timestamp_millis(),
    });
    peers.save()?;
    *PENDING_PAIRING.lock().unwrap() = None;
    writeln!(
        stream,
        "OK PAIR {} {}",
        crate::core::device_identity::local_device_id(),
        p.secret
    )?;
    stream.flush()?;
    Ok(())
}

fn handle_pair_v2(stream: &mut TcpStream, args: &[&str]) -> Result<()> {
    if args.len() < 3 {
        return write_err(stream, "PAIR2 needs code, device_id and device_name");
    }
    let code = args[0];
    let device_id = args[1];
    let device_name = args[2..].join(" ");
    let pending = PENDING_PAIRING.lock().unwrap().clone();
    let Some(p) = pending else {
        return write_err(stream, "pairing not active");
    };
    if p.code != code {
        return write_err(stream, "invalid code");
    }
    let peer_addr = stream.peer_addr()?.to_string();
    let mut peers = super::trusted_peers::TrustedPeersFile::load()?;
    peers.upsert(TrustedPeer {
        id: device_id.to_string(),
        device_name: device_name.clone(),
        lan_addr: peer_addr,
        pairing_secret: p.secret.clone(),
        trusted_at_ms: chrono::Utc::now().timestamp_millis(),
    });
    peers.save()?;
    *PENDING_PAIRING.lock().unwrap() = None;
    writeln!(
        stream,
        "OK PAIR2 {} {} {}",
        crate::core::device_identity::local_device_id(),
        p.secret,
        crate::core::device_identity::local_device_name(),
    )?;
    stream.flush()?;
    Ok(())
}

fn handle_push(stream: &mut TcpStream, args: &[&str]) -> Result<()> {
    if args.len() < 2 {
        return write_err(stream, "PUSH needs secret and len");
    }
    let secret = args[0];
    let len: usize = args[1].parse().context("PUSH len")?;
    if !auth_secret(secret) {
        return write_err(stream, "unauthorized");
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    let body = String::from_utf8(buf).context("push body utf8")?;
    let remote_host = stream.peer_addr()?.ip().to_string();
    if let Ok(v4) = crate::core::snapshot::decode_snapshot(&body) {
        let dir = snapshots_incoming_dir().join(&remote_host);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", v4.composer.composer_id));
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &body)?;
        std::fs::rename(&tmp, &path)?;
    }
    writeln!(stream, "OK PUSH")?;
    stream.flush()?;
    Ok(())
}

fn handle_pull(stream: &mut TcpStream, args: &[&str]) -> Result<()> {
    if args.is_empty() {
        return write_err(stream, "PULL needs secret");
    }
    let secret = args[0];
    if !auth_secret(secret) {
        return write_err(stream, "unauthorized");
    }
    let since_ms: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let host = local_hostname();
    let snap_dir = crate::core::paths::bettercursor_dir()
        .join("snapshots")
        .join(&host);
    let mut count = 0u32;
    if snap_dir.exists() {
        for entry in std::fs::read_dir(&snap_dir)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if mtime < since_ms {
                continue;
            }
            let body = std::fs::read_to_string(&path)?;
            writeln!(stream, "SNAP {}", body.len())?;
            stream.write_all(body.as_bytes())?;
            count += 1;
        }
    }
    writeln!(stream, "OK PULL {count}")?;
    stream.flush()?;
    Ok(())
}

fn auth_secret(secret: &str) -> bool {
    super::trusted_peers::TrustedPeersFile::load()
        .map(|f| f.peers.iter().any(|p| p.pairing_secret == secret))
        .unwrap_or(false)
}

fn write_err(stream: &mut TcpStream, msg: &str) -> Result<()> {
    writeln!(stream, "ERR {msg}")?;
    stream.flush()?;
    Ok(())
}

fn read_line(stream: &mut TcpStream, buf: &mut String) -> Result<()> {
    buf.clear();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0] as char);
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct PairResult {
    device_id: String,
    secret: String,
    device_name: String,
}

fn pair_v2_with_fallback(
    addr: &SocketAddr,
    code: &str,
    device_id: &str,
    device_name: &str,
) -> Result<PairResult> {
    match pair_v2(addr, code, device_id, device_name) {
        Ok(result) => Ok(result),
        Err(err) => {
            log::debug!("PAIR2 failed, falling back to legacy PAIR: {err:#}");
            pair_legacy(addr, code)
        }
    }
}

fn pair_v2(
    addr: &SocketAddr,
    code: &str,
    device_id: &str,
    device_name: &str,
) -> Result<PairResult> {
    let mut stream = TcpStream::connect_timeout(addr, Duration::from_secs(10))?;
    writeln!(stream, "{PROTO}")?;
    writeln!(stream, "PAIR2 {code} {device_id} {device_name}")?;
    stream.flush()?;
    let mut line = String::new();
    read_line(&mut stream, &mut line)?;
    parse_pair_v2_response(&line)
}

fn pair_legacy(addr: &SocketAddr, code: &str) -> Result<PairResult> {
    let mut stream = TcpStream::connect_timeout(addr, Duration::from_secs(10))?;
    writeln!(stream, "{PROTO}")?;
    writeln!(stream, "PAIR {code} bettercursor-peer")?;
    stream.flush()?;
    let mut line = String::new();
    read_line(&mut stream, &mut line)?;
    parse_pair_legacy_response(&line, &addr.to_string())
}

fn parse_pair_v2_response(line: &str) -> Result<PairResult> {
    if !line.starts_with("OK PAIR2 ") {
        return Err(anyhow!("pair rejected: {line}"));
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    let device_id = parts
        .get(2)
        .ok_or_else(|| anyhow!("missing device_id in PAIR2 response"))?;
    let secret = parts
        .get(3)
        .ok_or_else(|| anyhow!("missing secret in PAIR2 response"))?;
    let device_name = parts
        .get(4..)
        .map(|v| v.join(" "))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "bettercursor-peer".to_string());
    Ok(PairResult {
        device_id: (*device_id).to_string(),
        secret: (*secret).to_string(),
        device_name,
    })
}

fn parse_pair_legacy_response(line: &str, lan_addr: &str) -> Result<PairResult> {
    if !line.starts_with("OK PAIR ") {
        return Err(anyhow!("pair rejected: {line}"));
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    let device_id = parts
        .get(2)
        .ok_or_else(|| anyhow!("missing device_id in legacy pair response"))?;
    let secret = parts
        .get(3)
        .ok_or_else(|| anyhow!("missing secret in legacy pair response"))?;
    Ok(PairResult {
        device_id: (*device_id).to_string(),
        secret: (*secret).to_string(),
        device_name: super::trusted_peers::fallback_device_name(lan_addr),
    })
}

pub struct LanTcpTransport {
    peer: TrustedPeer,
}

impl LanTcpTransport {
    pub fn new(peer: TrustedPeer) -> Self {
        Self { peer }
    }

    fn peer_addr(&self) -> Result<SocketAddr> {
        self.peer
            .lan_addr
            .parse()
            .with_context(|| format!("parse lan_addr {}", self.peer.lan_addr))
    }

    async fn tcp_push(&self, body: &str) -> Result<()> {
        let addr = self.peer_addr()?;
        let secret = self.peer.pairing_secret.clone();
        let body = body.to_string();
        tokio::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
            writeln!(stream, "{PROTO}")?;
            writeln!(stream, "PUSH {secret} {}", body.len())?;
            stream.write_all(body.as_bytes())?;
            stream.flush()?;
            let mut resp = String::new();
            read_line(&mut stream, &mut resp)?;
            if !resp.starts_with("OK") {
                return Err(anyhow!("push rejected: {resp}"));
            }
            Ok(())
        })
        .await
        .context("join tcp push")??;
        Ok(())
    }

    async fn tcp_pull(&self, since_ms: i64) -> Result<Vec<RemoteSnapshot>> {
        let addr = self.peer_addr()?;
        let secret = self.peer.pairing_secret.clone();
        tokio::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
            writeln!(stream, "{PROTO}")?;
            writeln!(stream, "PULL {secret} {since_ms}")?;
            stream.flush()?;
            let mut out = Vec::new();
            loop {
                let mut line = String::new();
                read_line(&mut stream, &mut line)?;
                if line.starts_with("OK PULL") {
                    break;
                }
                if let Some(rest) = line.strip_prefix("SNAP ") {
                    let len: usize = rest.parse()?;
                    let mut buf = vec![0u8; len];
                    stream.read_exact(&mut buf)?;
                    let body = String::from_utf8(buf)?;
                    if let Ok(v4) = crate::core::snapshot::decode_snapshot(&body) {
                        out.push(RemoteSnapshot::V4(v4));
                    }
                }
            }
            Ok(out)
        })
        .await
        .context("join tcp pull")?
    }
}

#[async_trait]
impl Transport for LanTcpTransport {
    async fn push(&self, snap: &PushSnapshot) -> Result<PushReport> {
        let started = Instant::now();
        let body = snap.encode_body()?;
        self.tcp_push(&body).await?;
        Ok(PushReport {
            uuid: snap.uuid().to_string(),
            bytes_written: body.len() as u64,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn pull(&self, since_ms: i64) -> Result<Vec<RemoteSnapshot>> {
        self.tcp_pull(since_ms).await
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
        &self.peer.id
    }
}

fn rand_digit() -> u8 {
    (rand_bytes(1)[0] % 10) + b'0'
}

fn rand_bytes(n: usize) -> Vec<u8> {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64;
    let mut out = Vec::with_capacity(n);
    let mut x = seed;
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push((x & 0xFF) as u8);
    }
    out
}

fn local_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_pairing_has_six_digit_code() {
        let p = PendingPairing {
            code: "123456".into(),
            secret: "abc".into(),
            created_at_ms: 0,
        };
        assert_eq!(p.code.len(), 6);
    }

    #[test]
    fn parse_pair_v2_response_keeps_remote_name() {
        let got = parse_pair_v2_response("OK PAIR2 remote-id secret-1 macbook pro").unwrap();
        assert_eq!(
            got,
            PairResult {
                device_id: "remote-id".into(),
                secret: "secret-1".into(),
                device_name: "macbook pro".into(),
            }
        );
    }

    #[test]
    fn parse_pair_legacy_response_uses_addr_fallback_name() {
        let got =
            parse_pair_legacy_response("OK PAIR remote-id secret-1", "192.168.0.8:38472").unwrap();
        assert_eq!(got.device_name, "192.168.0.8");
    }
}
