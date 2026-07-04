#!/usr/bin/env bash
# fake-rsync.sh — mock rsync 二进制 for SshRsyncTransport 测试.
#
# 行为:
# - 真 rsync 调 ssh 子进程拉文件; 我们这边直接"模拟"成功 — 把目标目录
#   下面写一个 dummy *.json, 让 transport.pull() 能 walk 到.
# - argv 写到 /tmp/bettercursor-fake-rsync.log.
# - exit 0 (success).
#
# v0.2.6 first cut: 测 transport.pull() 只验证"rsync 被调用 + tmpdir 写了
# *.json → transport decode 走通". 跨设备真实 rsync 行为 (delta 传输、
# --include/exclude 语义) 留给 manual e2e.

set -u

LOG_FILE="${FAKE_RSYNC_LOG:-/tmp/bettercursor-fake-rsync.log}"

# argv 写日志.
{
  printf 'FAKE_RSYNC_INVOKED pid=%d ts=%s\n' "$$" "$(date +%s)"
  printf '  argv:'
  for a in "$@"; do
    printf ' %q' "$a"
  done
  printf '\n'
} >> "$LOG_FILE"

if [ "${FAKE_RSYNC_FAIL:-0}" = "1" ]; then
  echo "fake-rsync: simulated failure (FAKE_RSYNC_FAIL=1)" >&2
  exit 1
fi

# rsync argv 形如: -az --include=... --exclude=... -e ssh-proxy
#                  <remote> <local_dst>/
# 最后一个非 flag 参数就是 local destination. 写一个 dummy *.json 让
# transport.pull() walk 时拿到东西.
LOCAL_DST=""
for a in "$@"; do
  case "$a" in
    -*) ;;
    *)  LOCAL_DST="$a" ;;
  esac
done

if [ -z "$LOCAL_DST" ]; then
  echo "fake-rsync: cannot infer local destination from argv" >&2
  exit 2
fi

# 去掉末尾的 "/"
LOCAL_DST="${LOCAL_DST%/}"
mkdir -p "$LOCAL_DST"

# 写一个 dummy snapshot JSON (timestamp = 2000-01-01 epoch ms, 任意时间)
# 让 pull() 的 mtime 过滤至少能拿到一些东西.
cat > "$LOCAL_DST/dummy-uuid.json" <<'EOF'
{"uuid":"dummy-uuid","last_updated_at_ms":1700000000000,"host":"fake-host","project_slug":"fake-slug","project_path":"/fake/path","source_path":"/fake/file.jsonl","text_preview":"hello fake","bubble_count":1}
EOF

exit 0