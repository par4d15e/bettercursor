//! 本机设备标识：稳定 device_id + 当前 hostname。

use std::path::PathBuf;

/// 读取或生成稳定的本机 device_id，持久化到 `~/.bettercursor/device_id`。
pub fn local_device_id() -> String {
    let path = device_id_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let id = hex::encode(rand_bytes(16));
    let _ = std::fs::create_dir_all(crate::core::paths::bettercursor_dir());
    let _ = std::fs::write(&path, &id);
    id
}

/// 当前主机名。失败时回退到 `"unknown"`，避免阻断配对 / 广播流程。
pub fn local_device_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn device_id_path() -> PathBuf {
    crate::core::paths::bettercursor_dir().join("device_id")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_device_id_is_stable_within_same_home() {
        let _guard = crate::core::paths::test_home_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BETTERCURSOR_TEST_HOME", dir.path());
        let first = local_device_id();
        let second = local_device_id();
        std::env::remove_var("BETTERCURSOR_TEST_HOME");
        assert!(!first.is_empty());
        assert_eq!(first, second);
    }
}
