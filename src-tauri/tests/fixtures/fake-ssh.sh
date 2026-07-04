#!/usr/bin/env bash
# fake-ssh.sh — 用于 SshRsyncTransport::with_bins 测试的 mock ssh 二进制.
#
# 行为:
# - 把 argv (含 stdin 不可及) 写到 /tmp/bettercursor-fake-ssh.log (一行 JSON-ish).
# - exit 0, stdout "ok\n" (跟真 ssh 行为一致 — 命令 stdout 透传).
# - 支持环境变量 FAKE_SSH_FAIL=1 模拟失败 (exit 1 + stderr).
#
# 用法:
#   SshRsyncTransport::with_bins(peer, "tests/fixtures/fake-ssh.sh", "rsync")
#
# 不要把它放在 PATH 里跑; 测试只通过 with_bins 显式注入路径.

set -u

LOG_FILE="${FAKE_SSH_LOG:-/tmp/bettercursor-fake-ssh.log}"

# 把 argv 写进 log (用 null 分隔方便后续解析).
{
  printf 'FAKE_SSH_INVOKED pid=%d ts=%s\n' "$$" "$(date +%s)"
  printf '  argv:'
  for a in "$@"; do
    printf ' %q' "$a"
  done
  printf '\n'
} >> "$LOG_FILE"

if [ "${FAKE_SSH_FAIL:-0}" = "1" ]; then
  echo "fake-ssh: simulated failure (FAKE_SSH_FAIL=1)" >&2
  exit 1
fi

echo "ok"
exit 0