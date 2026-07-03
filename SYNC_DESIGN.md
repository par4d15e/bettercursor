# bettercursor — SYNC_DESIGN (v0.2+ 后续能力设计稿)

> 文档目的: 设计 v0.1 之后要加的能力 — **本地同机 sync**、**跨设备 sync**、**对话记录展开**、**修 orphan session** 等. 配套 [PRD.md](PRD.md) §0.5 (v0.1 没做的) 和 §7 (路线图).
>
> v0.1 已经实现只读 session 查看器 ([PRD §0](PRD.md)), 本文档假设读者已熟悉 v0.1 架构 (`src-tauri/src/core/{paths,storage,canonical}.rs` + `src/components/*`).
>
> **状态**: 设计稿, **未实现**. 用户拍板后才开干.

---

## 0. 为什么需要 sync

**v0.1 的痛点**: 用户打开 bettercursor 看到 17 条 session, 但**只能在原来产生它的端 resume**. 例如:
- Linux CLI 上 `cursor-agent` 创建的 session, 在 Mac Electron 看不到
- Mac Electron 创建的 session, 在 Linux CLI 看不到 (虽然 Layer 1 JSONL 在 Linux 上)
- Linux CLI 与 Linux Electron Desktop 同机, 互相看不到对方的 session

**用户实际工作流** (基于 goal.md):
1. Mac 跟 agent 聊 → 中途 SSH 到 Linux → 想在 Linux CLI 继续
2. Linux CLI 跟 agent 聊 → 回到 Mac → 想在 Mac Sidebar 继续
3. 写完代码 → 切到 Linux 编译 → 跑测试 → agent 应该记得上下文

**sync 的目标**: 让**任意一端创建的 session**, 在**任意一端可见 + 可 resume**, 用户无感.

---

## 1. 能力矩阵

| 场景 | 描述 | 难度 | 估时 |
|------|------|------|------|
| **A. 本机 Desktop↔CLI** | Linux 上同时跑 cursor-agent CLI + Electron Desktop, 互相 sync | 中 | 2-3 天 |
| **B. 跨设备 Mac↔Linux** | Mac 创建的 session 自动出现在 Linux (反之亦然), 通过 Tailscale mesh | 高 | 4-5 天 |
| **C. 修 orphan session** | 修 `meta[0].latestRootBlobId = ""`, 让 `cursor-agent --resume` 不再静默失败 | 低 | 0.5 天 (Python 已实现, 移植) |
| **D. 对话记录展开** | 读 store.db blobs + JSONL messages, 完整渲染对话气泡 | 中 | 1-2 天 |
| **E. 自动同步 toggle** | UI 按钮启停后台 sync loop, 5min 间隔 + notify 触发 | 中 | 1 天 |
| **F. 手动单次同步** | UI "Sync Now" 按钮, 立即触发一次 sync | 低 | 0.5 天 (接在 E 后面) |
| **G. delete session** | 从 store.db + JSONL 删一条 session (Layer 3 不动) | 低 | 0.5 天 |

**v0.2 推荐**: C → D → E+F → A → B → G (按用户价值排)

---

## 2. 整体架构 (v0.2+ 演进)

```
┌─────────────────────────────────────────────────────────────────────┐
│  本机: Mac 或 Linux (Tauri 桌面应用, v0.1 基础上加)                  │
│                                                                     │
│  ┌────────────────────┐         ┌─────────────────────────────┐   │
│  │  React Frontend    │ ←invoke→│  Rust Backend (Tauri cmd)   │   │
│  │  (WebView)         │         │  ───────────────────────    │   │
│  │  • SessionTree     │         │  • core::paths/storage/     │   │
│  │  • SessionDetail   │         │    canonical (v0.1)         │   │
│  │  • SourceBadge     │         │  • core::snapshot (NEW)     │   │
│  │  • SyncToggle ⭐   │         │  • core::layer2/3 writer    │   │
│  │  • SyncNowBtn ⭐   │         │  • core::blob_dag (NEW)     │   │
│  │  • MessageList ⭐  │         │  • core::sync (NEW orchestr)│   │
│  │  • DeleteBtn ⭐    │         │                             │   │
│  └────────────────────┘         └─────────────────────────────┘   │
│                                          │                          │
│                                          ↓ (v0.2+ 新增)             │
│              ┌────────────────┬──────────────────┬──────────────┐  │
│              │ Layer 1 (JSONL)│ Layer 2 (store.db)│ Layer 3      │  │
│              │   只读          │   读+写 ⭐         │   读+写 ⭐    │  │
│              └────────────────┴──────────────────┴──────────────┘  │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │  sync daemon (tokio task, ⭐ NEW)                            │   │
│  │   • 5min 间隔轮询 + notify 监听 store.db/state.vscdb 变更   │   │
│  │   • 触发: 比对本地 + 远端 snapshot, 拉/推增量              │   │
│  │   • 远端 = Tailscale mesh 上 Linux daemon 或 Mac client     │   │
│  └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
                                  ↕ (仅场景 B)
                                  ↕ Tailscale (100.x.x.x) + SSH
┌─────────────────────────────────────────────────────────────────────┐
│  远端: 另一台 Mac 或 Linux (同样跑 Tauri + daemon)                  │
└─────────────────────────────────────────────────────────────────────┘
```

⭐ = v0.2+ 新增

---

## 3. 核心数据格式: Snapshot

Snapshot 是 sync 系统的**通用数据交换格式**. 任意端 → 任意端, 都先把本地 session 序列化成 Snapshot, 再传到对面.

### 3.1 Snapshot schema (v3, 跟 cursaves 兼容)

```rust
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,                    // schema 版本, 当前 3
    pub exported_at: i64,                // ms epoch
    pub source_endpoint: SourceEndpoint, // NEW (cursaves 没有)

    pub composer_id: String,             // UUID
    pub composer_data: serde_json::Value,

    pub content_blobs: HashMap<String, String>,  // blob_id -> b64
    pub bubble_entries: HashMap<String, serde_json::Value>,  // bubble_id -> JSON
    pub checkpoints: HashMap<String, String>,    // checkpoint_id -> b64

    pub agent_blobs: HashMap<String, String>,    // SHA256 hex -> b64 (完整 DAG)
    pub transcript: Vec<serde_json::Value>,      // 全部消息
    pub message_contexts: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
pub struct SourceEndpoint {
    pub host: String,            // hostname
    pub os: String,              // "macos" | "linux" | "windows"
    pub user: String,            // $USER
    pub endpoint_kind: String,   // "linux_cli" | "mac" | "linux_desktop"
    pub cursor_version: Option<String>,
}
```

### 3.2 编码

- **gzip 压缩**整个 JSON, 写 `.json.gz`
- 路径: `~/.bettercursor/snapshots/<endpoint_host>/<composer_id>-<timestamp>.json.gz`
- v0.1 暂不导出 Snapshot, 端口先照搬 [vendored/cursaves/cursor_saves/snapshot.py](../vendored/cursaves/cursor_saves/snapshot.py) 的 codec

### 3.3 写入时的"修 root"流程

Cursor 的 store.db 里 `meta[0].latestRootBlobId` 必须**非空**才能 `--resume`. c1ea7999 就是因为这个字段为空导致 `--resume` 静默失败.

**修复算法** (`core/blob_dag.rs`):
1. 读 store.db 全部 blobs (80 个左右)
2. 解析每个 blob 的 protobuf, 找包含 32-byte SHA256 ref 的 tree node
3. 计算 transitive coverage: 哪个 blob 被引用最多 → 大概率是 root
4. 写 `meta[0].latestRootBlobId = <root_id>`
5. 如果 root 找不到, 失败回滚, 报 "incomplete snapshot"

**Python 参考**: [`bettercursor/blob_dag.py`](bettercursor/blob_dag.py) 已实现 + 端到端 PASS. 端口到 Rust 用手写 varint decoder (~200 行, 不引 prost 依赖).

---

## 4. Tauri Command API (v0.2+)

### 4.1 新增 commands

| Command | 参数 | 返回 | 阶段 |
|---------|------|------|------|
| `sync_now` | — | `SyncReport` (写入了多少 blob, 修了几条 root) | v0.2 F |
| `set_auto_sync` | `enabled: bool` | `()` | v0.2 E |
| `get_sync_status` | — | `{ enabled, last_run_at, next_run_in_sec, last_report }` | v0.2 E |
| `get_messages` | `uuid: String` | `Vec<Message>` (对话气泡) | v0.2 D |
| `delete_session` | `uuid: String` | `DeleteReport` | v0.2 G |
| `fix_orphans` | — | `Vec<OrphanFixReport>` | v0.2 C |

### 4.2 `sync_now` 设计

```rust
#[derive(Serialize)]
pub struct SyncReport {
    pub started_at: i64,
    pub finished_at: i64,
    pub scanned: u32,
    pub imported: u32,           // 写到 Layer 2 的
    pub layer2_writes: u32,      // 写 store.db 的次数
    pub layer3_writes: u32,      // 写 state.vscdb 的次数
    pub roots_fixed: u32,        // 修了多少 latestRootBlobId
    pub errors: Vec<String>,     // 失败的 session (不中断整体)
}

#[tauri::command]
async fn sync_now(state: State<'_, AppState>) -> Result<SyncReport, String> {
    core::sync::run_once().await.map_err(|e| e.to_string())
}
```

### 4.3 UI 新增组件

```
src/components/
├── SyncToggle.tsx        ← ⭐ v0.2 (E)
├── SyncNowButton.tsx     ← ⭐ v0.2 (F)
├── SyncStatusBadge.tsx   ← ⭐ v0.2 (E) — "已同步 2 分钟前" / "已停止"
├── MessageList.tsx       ← ⭐ v0.2 (D)
├── MessageBubble.tsx     ← ⭐ v0.2 (D)
└── DeleteButton.tsx      ← ⭐ v0.2 (G)
```

UI 嵌入位置:
- `SessionDetail` 顶部工具栏加 `<SyncToggle>` (跟 cc-switch 的"添加 Provider"按钮同位置)
- `SessionDetail` 底部加 `<MessageList>` (替换 v0.1 的"v0.2 计划"占位符)
- `SessionDetail` "删除会话" 按钮从 disabled 改成 enabled (接 `delete_session`)

---

## 5. 后台同步循环 (v0.2 E)

### 5.1 架构

```
Tauri App 启动
    ↓
setup() 钩子
    ├─ AppState::new()
    └─ tokio::spawn(sync::daemon_loop)
         │
         └─ loop {
              if !state.auto_sync { sleep(5s); continue; }
              let r = sync::run_once().await;
              log::info!("sync: scanned={}, imported={}", r.scanned, r.imported);
              state.last_report = Some(r);
              state.last_run_at = Some(now());
              sleep(state.interval);  // 默认 5min
            }
    ↓
用户 toggle ON/OFF
    ↓
set_auto_sync(true/false) → 改 state.auto_sync → daemon loop 立即感知
```

### 5.2 触发加速: `notify` crate

仅靠 5min 间隔太慢. 加 `notify` 监听 store.db / state.vscdb 变更:
- Linux: inotify (FSEvents 不支持, polling 兜底)
- macOS: FSEvents (原生, 几乎无延迟)
- Windows: ReadDirectoryChangesW

```rust
use notify::{Watcher, RecursiveMode, EventKind};

let (tx, rx) = std::sync::mpsc::channel();
let mut watcher = notify::recommended_watcher(tx)?;
watcher.watch(&paths::global_db_path()?, RecursiveMode::NonRecursive)?;
watcher.watch(&paths::chats_dir()?, RecursiveMode::Recursive)?;

// 后台线程
std::thread::spawn(move || {
    for event in rx {
        if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            tx_oneshot.send(()).ok();
        }
    }
});
```

监听触发 → 通过 `tokio::sync::Notify` 唤醒 daemon loop, 跳过本次 sleep.

### 5.3 资源预算 (v0.5 US-5 验收)

- idle CPU: < 0.5%
- 内存: < 50 MB (Tauri + tokio + watch + rusqlite pool)

实测: notify 监听单文件, idle 时 < 0.1% CPU. tokio task 5min sleep 不耗资源.

---

## 6. 跨设备 sync (场景 B)

### 6.1 网络: Tailscale mesh

跟 cc-switch / cursync-import 一致, **走 Tailscale 100.x.x.x 私有网络**, 不走公网.

**前置条件** (用户已具备, 见 [mihomo-tailscale-fakeip-conflict.md](mihomo-tailscale-fakeip-conflict.md)):
- Mac + Linux 都在 Tailscale mesh 上
- Mac 上跑 `tailscale ip` 拿到 100.x.x.x
- Linux 上跑 `tailscale ip` 拿到 100.x.x.y

### 6.2 SSH 反向推送 (跟原 Phase 3 设计一致)

**架构**:
- **Linux 端**: 跑 daemon (Tauri app 里的 tokio task, 不是独立进程), 监听 SSH 命令
- **Mac 端**: launchd plist 每 5 分钟触发, 调 `ssh linux "..."` 拉 + 推

**Linux daemon 暴露的 SSH 命令** (走 `authorized_keys` + `force-command`):
```bash
# Mac 拉取
ssh linux 'bettercursor-syncd export --project=enenzuo' > /tmp/pull.json

# Mac 推送
ssh linux 'bettercursor-syncd import' < /tmp/push.json
```

**Tauri 实现** (不走 SSH, 走 Tailscale 直接 HTTP):
- Linux daemon 跑一个轻量 HTTP server (axum, 127.0.0.1:7342)
- Mac 客户端 POST `http://linux.tailnet.ts.net:7342/api/import` (推送)
- Mac 客户端 GET `http://linux.tailnet.ts.net:7342/api/export?project=...` (拉取)
- mTLS 或 Tailscale ACL 鉴权 (不用密码)

### 6.3 数据流

```
[Mac 启动 bettercursor]
    ↓
[setup: 跟 Linux HTTP 握手, 拉取 Linux canonical]
    ↓
[合并 Mac 本地 + Linux 远端, 渲染 SessionTree]
    ↓
[Mac 用户开新 session → Layer 3' 写入]
    ↓
[后台 5min 触发: POST Linux /api/import < Mac snapshot]
    ↓
[Linux daemon: 比对, last-writer-wins 写 Layer 2/3]
    ↓
[修 root, 返回 SyncReport]
    ↓
[Mac emit 'sessions-updated' → UI 刷新]
```

### 6.4 冲突解决

| 场景 | 策略 |
|------|------|
| 同一 UUID 两端都改, Mac 新 | Mac 覆盖 Linux (last-writer-wins) |
| 同一 UUID 两端都改, Linux 新 | Linux 覆盖 Mac, Mac 收到 SyncReport 后撤回 |
| 同一 UUID 双方都改, 时间接近 (5min 内) | 备份双方到 `~/.bettercursor/archive/<uuid>/<timestamp>.json`, 用户手动合并 |
| 一端有, 另一端没 | 缺的那端 import 即可 |
| Bubble ID set 双方不同 | merge 并集, 重复取 content hash 相同 |

### 6.5 退出策略

如果 Tailscale 不可用 (用户没装 / mesh 没起):
- v0.2 默认降级为本地 sync (场景 A only)
- UI 上显示 "Tailscale 未连接, 仅本机 sync"
- Mac 单独跑, 看不到 Linux (符合 v0.1 行为)

---

## 7. 对话记录展开 (场景 D)

### 7.1 读取逻辑

`get_messages(uuid)`:
1. 根据 `uuid` 在哪个 layer 找
2. **Layer 2 找到** (store.db 里有):
   - 读 `meta[0]` → `latestRootBlobId` → 沿 DAG 走
   - 找所有 "type: 1" (user) 和 "type: 2" (assistant) 的 JSON leaf blob
   - 按时间戳排序, 渲染
3. **Layer 1 找到** (JSONL):
   - 直接 read 整个 .jsonl, 解析成 messages
4. **Layer 3 找到** (state.vscdb):
   - 读 `cursorDiskKV.bubbleId:<uuid>:<bid>` keys
   - 拼成 messages
5. **多源**: merge 按 timestamp, 同 timestamp 取最长 content

### 7.2 性能

- 单 session 80 个 blob, 总大小 ~1-5 MB, 解析 < 100ms
- 加载用 `tokio::task::spawn_blocking`, 不阻塞 UI thread
- 前端用虚拟滚动 (@tanstack/react-virtual) 渲染长对话

### 7.3 UI

跟 cc-switch 的"对话记录 2402" 一致:
```
┌──────────────────────────────────────────┐
│  对话记录 (11)                            │
│  ┌──────────────────────────────────┐    │
│  │ User · 2026/7/2 17:10:54          │    │
│  │ 读完 README.md, ...               │    │
│  └──────────────────────────────────┘    │
│  ┌──────────────────────────────────┐    │
│  │ AI · 2026/7/2 17:10:55            │    │
│  │ 已读完. 根据 §3, ...              │    │
│  └──────────────────────────────────┘    │
│  ...                                      │
└──────────────────────────────────────────┘
```

---

## 8. 写 store.db / state.vscdb 细节

### 8.1 Layer 2 写 (cursor-agent 的 store.db)

`core/layer2.rs::import_snapshot(snapshot)`:
1. 决定目标 cwd (从 snapshot.source_endpoint 或 git remote 推断)
2. 计算 chat_root = MD5(cwd)
3. 创建 `~/.cursor/chats/<chat_root>/<composer_id>/` 目录
4. 写 meta.json (schemaVersion, hasConversation, title, createdAt)
5. 写 prompt_history.json (["/resume"])
6. 写 store.db:
   - 创建表 `blobs(id TEXT, data BLOB)`, `meta(key TEXT, value TEXT)`
   - 写 80 个 agentBlobs (snapshot.agent_blobs)
   - 写 meta[0] = JSON {agentId, name, latestRootBlobId: ""}
   - **修 root**: 调 `core/blob_dag::fix_latest_root()`

**Python 参考**: [`bettercursor/layer2.py`](bettercursor/layer2.py) 端到端 PASS. 端口照搬.

### 8.2 Layer 3 写 (Electron 的 state.vscdb)

`core/layer3.rs::import_snapshot(snapshot)`:
1. 决定目标 workspace (从 snapshot.composer_data.workspaceIdentifier)
2. 写 `ItemTable.composer.composerData` (单 blob, 旧 Cursor 格式)
3. 写 `cursorDiskKV.composerData:<uuid>` (新格式)
4. 写 `cursorDiskKV.bubbleId:<uuid>:<bid>` 每条 bubble
5. **路径修复**: 改写 JSON 里的 `fsPath` 字段, 跟当前 machine 匹配 (避免跨设备 stale path)

**Python 参考**: [`bettercursor/layer3.py`](bettercursor/layer3.py) 已实现 (190 行). 端口照搬.

### 8.3 写前安全检查

- ✅ backup 目标 DB (`backup_db(store_db)` → `store.db.backup_<ts>`)
- ✅ 写后 `PRAGMA integrity_check`, 失败回滚
- ✅ WAL-safe: 写期间若 Cursor 正在用, retry 3 次, 还失败就报给 UI

---

## 9. 阶段拆解 (v0.2 路线图)

### v0.2.1 — 修 orphan + delete (低风险, 1 天)

| 任务 | 估时 |
|------|------|
| 端口 `bettercursor/blob_dag.py` → `src-tauri/src/core/blob_dag.rs` (protobuf parser + fix_latest_root) | 0.5 天 |
| Tauri command: `fix_orphans` (扫所有 store.db, 自动修 root) | 0.25 天 |
| Tauri command: `delete_session` (Layer 2 + JSONL, 不动 Layer 3) | 0.25 天 |
| UI: DeleteButton 启用 + FixOrphans 菜单项 | 0.25 天 |

**价值**: 解决 c1ea7999 这种历史 orphan 死数据, 让历史 session 重新可 resume.

### v0.2.2 — 对话记录展开 (1.5 天)

| 任务 | 估时 |
|------|------|
| Tauri command: `get_messages` (读 3 层, merge) | 1 天 |
| UI: `<MessageList>` + `<MessageBubble>` + 虚拟滚动 | 0.5 天 |

**价值**: 用户能在 bettercursor 里**直接看 agent 聊了什么**, 不切到 Cursor.

### v0.2.3 — 本机 sync loop (2 天)

| 任务 | 估时 |
|------|------|
| 端口 `bettercursor/snapshot.py` → `core/snapshot.rs` (gzip codec) | 0.5 天 |
| 端口 `bettercursor/layer2.py` 写函数 → `core/layer2.rs` | 0.25 天 |
| 端口 `bettercursor/layer3.py` 写函数 → `core/layer3.rs` | 0.25 天 |
| `core/sync.rs` 编排 (snapshot → import → fix root → 报告) | 0.5 天 |
| `sync::daemon_loop` (tokio task, 5min 间隔) | 0.25 天 |
| notify crate 监听 + 触发加速 | 0.25 天 |
| Tauri commands: `sync_now` / `set_auto_sync` / `get_sync_status` | 0.25 天 |
| UI: SyncToggle / SyncNowButton / SyncStatusBadge | 0.25 天 |

**价值**: 场景 A. Linux 上同机 Desktop + CLI 自动互见.

### v0.2.4 — 跨设备 sync (4-5 天)

| 任务 | 估时 |
|------|------|
| Linux daemon: axum HTTP server, /api/{import,export} endpoints | 1 天 |
| Tailscale mTLS 鉴权 | 0.5 天 |
| Mac client: 配对 (首次输 100.x.x.x), launchd plist | 1 天 |
| 增量 export (`since=<ts>`) + 冲突检测 (last-writer-wins) | 1 天 |
| 跨设备状态合并 UI 提示 ("Mac 3 分钟后同步") | 0.5 天 |
| 端到端测试: Mac 创建 → Linux 5min 内可见 | 1 天 |

**价值**: 场景 B. Mac + Linux 互见.

### v0.2.5 — 打包分发 (1 天)

| 任务 | 估时 |
|------|------|
| `pnpm tauri build` 配置 (Linux: deb + AppImage, Mac: dmg) | 0.5 天 |
| Mac cross-compile from Linux (`--target aarch64-apple-darwin`) | 0.25 天 |
| README + 安装说明 | 0.25 天 |

---

## 10. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| Cursor 升级改 store.db schema | 中 | 高 | 写时 backup, 失败回滚; schema 不匹配时整 session 标 error |
| 跨设备写冲突 (同时改一个 session) | 低 | 中 | last-writer-wins + archive 备份双方 |
| Tailscale 断网 / 节点离线 | 中 | 低 | 离线期间本地继续工作, 上线后补偿同步 |
| sync loop 写时 Cursor 正在用 | 中 | 中 | WAL-safe 模式 + retry 3 次 + 写后 integrity_check |
| protobuf 解析复杂, DAG 推算错误 | 低 | 高 | 用真实 c1ea7999 snapshot 跑集成测试, Python 参考比对 |
| 后台 loop 占资源 (idle CPU) | 低 | 低 | 5min 间隔 + notify 加速 + tokio cooperative |
| 100+ session 同步慢 (首次) | 中 | 低 | 增量 export (since=<ts>), 全量仅首次 |

---

## 11. 退出策略 (如果 sync 做不下去)

| 场景 | 回退 |
|------|------|
| 写 store.db 太复杂, protobuf 解析坑太多 | 退回到 v0.1 + delete (无 sync). Cursor 的"导出功能"自己出 snapshot. |
| Tailscale mesh 不可用 | 改 SSH 反向推送 (Python 已有经验, [BACKGROUND §6.5](BACKGROUND.md)) |
| 跨设备 sync 永远做不完 | 锁定 v0.2.3 (本机 sync), 跨设备留 v0.3+ |
| 用户改主意, 不要 sync 了 | 回到 v0.1, 删除 v0.2 代码. PRD §0 是 v0.1 真相. |

---

## 12. 关键参考

| 主题 | 已有参考 |
|------|---------|
| Snapshot codec | [`bettercursor/snapshot.py`](bettercursor/snapshot.py) 198 行, 跟 cursaves v3 兼容 |
| 修 root 算法 | [`bettercursor/blob_dag.py`](bettercursor/blob_dag.py) 188 行, 端到端 PASS |
| Layer 2 写 | [`bettercursor/layer2.py`](bettercursor/layer2.py) 183 行, 端到端 PASS |
| Layer 3 写 | [`bettercursor/layer3.py`](bettercursor/layer3.py) 189 行 |
| 冲突检测 | [`bettercursor/conflict.py`](bettercursor/conflict.py) 96 行, 5-way enum |
| Tailscale 设置 | [mihomo-tailscale-fakeip-conflict.md](mihomo-tailscale-fakeip-conflict.md) |
| 调研考古 | [BACKGROUND.md](BACKGROUND.md) 471 行 |
| 产品需求 | [PRD.md](PRD.md) |
| 实施计划 | [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) |
| Cursor 存储路径 | [vendored/cursaves/cursor_saves/paths.py](../vendored/cursaves/cursor_saves/paths.py) |

---

## 13. 决策待用户拍板

- [ ] **v0.2 启动顺序**: 1→2→3 (orphan→对话→sync) 还是 1→3→2 (orphan→sync→对话)?
- [ ] **本机 sync** vs **跨设备 sync**: 哪个先?
- [ ] **delete**: v0.2.1 加, 还是推迟到 v0.3?
- [ ] **Tailscale 强制**: 如果用户没装, 降级还是直接报错?
- [ ] **冲突解决 UX**: 自动 last-writer-wins 够用, 还是要弹窗让用户选?
- [ ] **snapshot 格式**: 跟 cursaves 100% 兼容 (好复用), 还是自己定 (好演化)?

拍板后, 把决策补到 PRD §0.5 和路线图 §7, 然后开干.
