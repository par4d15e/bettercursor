# 跨设备接力 E2E 验证手册 (v0.3.1)

本手册覆盖 **Phase A (SSH 高级模式)** 与 **Phase B (LAN 默认模式)** 的双机验证步骤。

## 前置条件

- 两台机器均安装 bettercursor v0.3.1+
- Cursor **关闭**（或至少不在写入目标 session 的 L3），否则 L2/L3 apply 可能被跳过
- 两台机器上均有可扫描到的 Cursor session（含 bubbles）

---

## 模式 A：SSH/rsync (T2b 高级)

### 1. 配置 `~/.bettercursor/transports.json`

机器 A（推送方）示例：

```json
{
  "peers": [
    {
      "id": "linux-box",
      "kind": "ssh",
      "host": "user@192.168.1.20",
      "port": 22,
      "identity_file": "~/.ssh/id_ed25519",
      "remote_snap_dir": "~/.bettercursor/snapshots_incoming",
      "remote_hostname": "linux-box"
    }
  ]
}
```

机器 B 需配置反向 peer（或对称配置），并确保 SSH 免密可用：

```bash
ssh -o BatchMode=yes user@192.168.1.10 true
```

### 2. 探活

在 bettercursor 中打开 **跨设备同步** 对话框，或 devtools：

```js
await window.__TAURI__.core.invoke('transport_test', { peerId: 'linux-box' })
```

期望 `ok: true`。

### 3. Push v4

选中一条 session，对 trusted/SSH peer 执行 push：

```js
await window.__TAURI__.core.invoke('transport_push', { uuid: '<session-uuid>', peerId: 'linux-box' })
```

期望 `bytes_written > 0`。

### 4. Pull + apply

在机器 B：

```js
await window.__TAURI__.core.invoke('transport_pull', { peerId: 'macbook', sinceMs: 0 })
```

验证：

1. `~/.bettercursor/unified.db` 含该 uuid 的 bubbles
2. Cursor L2 (`store.db`) / L3 (`state.vscdb`) 可 resume 该 session
3. bettercursor UI 刷新后可见同步来的 session

---

## 模式 B：LAN TCP + mDNS (T2a 默认)

### 1. 启动与发现

两台机器均启动 bettercursor（后台 sync loop 会自动启动 LAN 服务并广播 mDNS）。

在机器 A：打开 **跨设备同步** → **显示配对码**，记下 6 位码与端口。

在机器 B：**附近设备** 列表应出现机器 A；输入配对码 → **配对**。

配对结果写入 `~/.bettercursor/trusted_peers.json`（双方各一条）。

### 2. 自动 / 手动同步

- **自动**：后台每 5 分钟向 trusted peers push 有 bubbles 的 session；失败入 `~/.bettercursor/outbox/<peer_id>/`
- **手动**：在已信任设备行点击 **推送** / **拉取**

### 3. 验证接力

1. 机器 A 在 Cursor 中聊一半
2. 机器 B pull（或等待后台 sync）
3. 机器 B 在 Cursor / bettercursor 中 resume，气泡内容一致

### 4. 冲突

若双方同时编辑同一 session 产生 Diverged：

1. 打开 **同步冲突** 对话框
2. 查看 `unresolved_conflicts` 列表
3. 选择 **接受合并** 或 **跳过**

---

## 故障排查

| 现象 | 检查 |
|------|------|
| mDNS 无设备 | 同网段？防火墙放行 UDP 5353 / TCP LAN 端口？ |
| 配对失败 | 配对码是否过期（重新显示）？端口是否正确？ |
| pull 后 unified 有数据但 Cursor 无 | Cursor 是否在运行？查看日志 `transport_pull L2/L3 apply skipped` |
| SSH push 失败 | `transport_test` 错误信息；`remote_snap_dir` 权限 |
| outbox 堆积 | 对端离线或 LAN 不可达；恢复后等 5min flush 或手动 push |

---

## 相关单测

```bash
cd src-tauri && cargo test --lib transport
cd src-tauri && cargo test --lib discovery
cd src-tauri && cargo test --lib conflict
```
