# bettercursor — SYNC_DESIGN (v0.3+ 跨设备能力设计稿)

> 文档目的: 设计 v0.2 之后要加的能力 — **`~/.bettercursor/unified.db` 本机缓存**、**snapshot codec**、**多端接力**、**冲突解决与 3-way UI**、**用户自管备份**. 配套 [PRD.md](PRD.md) §0.6 (现状) 和 §7 (路线图).
>
> v0.1 已经实现只读 session 查看器 ([PRD §0.1](PRD.md)), v0.2-alpha + v0.2.1 已落地 (见 §0.5 callout). 本文档聚焦 **v0.3+ 跨设备 sync 架构**.
>
> 读者画像: 已熟悉 v0.1 架构 (`src-tauri/src/core/{paths,storage,canonical}.rs` + `src/components/*`), 想知道"为 v0.2.x 之后做设计时要参考什么约束".
>
> **状态**: 设计稿. v0.2-alpha + v0.2.1 已落地 (见 §0.5 callout). §3 unified.db / §4 transport 适配器 / §6 冲突解决 / §7 lock module 升级 仍是待拍板.
>
> **章节 map**:
> §0 背景与现状 → §1 核心架构 → §2 Snapshot codec → §3 unified.db schema → §4 传输适配器层 → §5 SSH/rsync (default) → §6 冲突解决 → §7 锁画像 → §8 多端接力 → §9 Cursor 集成 → §10 路线图 → §11 文件清单 (**含 §11.5 vendored 借鉴索引**) → §12 决策 → §13 风险.

### Reading guide: 旧章节 → 新章节 映射表

> 本次 (2026-07-04) 整体重写后, 旧 SYNC_DESIGN 的章节号已变. [PRD.md](PRD.md) / [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) 里旧链接仍然有效指向本文件, 但需要查表. 后续如果同步更新 cross-ref, 直接搜 grep -n "SYNC_DESIGN" 改成本表的"新章节号".

| 旧 SYNC_DESIGN 章节号 (2026-07-03) | 内容 | 本 doc 新章节号 |
|-----------------------------------|------|----------------|
| §0 为什么需要 sync + §0.5 v0.2-alpha 已落地 | 背景 + 已落地表 | §0 + §0.5 callout |
| §1 能力矩阵 | 7 场景估时表 | 删除 (跟 §10 路线图重叠, 路线图更准) |
| §2 整体架构 | 老 ASCII 框图 | §1 (但 framing 转向: unified.db 是 cache, 不是 hub) |
| §3 Snapshot schema v3 + gzip | 老 v3 gzip codec | **§2 (修订为 v4 plain text)** |
| §3.3 修 root 流程 | protobuf walker | (已在 sync.rs:612 inline; §9.6 提, 不再单独列) |
| §3.4 Cursor 数据 quirk | 5 个实测 quirk | §2.5 (移到 §2 内) |
| §4 Tauri Command API + §4.1 新增 commands | v0.2.3 待做的 commands | 删除 (老 command 表已跟当前 lib.rs 不同, 看 §0.5 callout + lib.rs 实测) |
| §4.3 sync_session_layer23 v0.2-alpha 实现 | 6 步编排 | 收口在 §0.5 callout + §9.1 写流程 (不要重复) |
| §5 后台同步循环 + §5.2 notify | tokio daemon_loop 设计 | (删除; §10 路线图 v0.2.3 是它, 但本文件不重写 tokio 细节) |
| §6.1 Tailscale mesh + §6.2 SSH 反向推送 + §6.4 冲突 | 旧 Tailscale + LWW 4-way | **拆分** → §4 (Tailscale 降级为 T5) + §5 (SSH) + §6 (LWW + 5-way) |
| §7 对话记录展开 | 读 3 层 merge | 删除 (跟 v0.2.2 路线图一对一; §10 提) |
| §8 写 store.db / state.vscdb 细节 | L2/L3 write 函数 | §9 (简化成 reference, 不重列 sync.rs 已写的) |
| §9 阶段拆解 (v0.2 路线图) | 5 个 milestone | **§10 (重排**: v0.2.1 ✅, v0.2.2/v0.2.3/v0.2.4 待做, v0.3.0 大版本) |
| §10 风险 | 7 类风险 | §13.1 (压成 5 类, 删除 cursor 升级类的细化) |
| §11 退出策略 | 5 类回退 | 并入 §13.2 |
| §12 决策 (v0.2-alpha + 待) | 决策列表 | **§12 (扩展到 10 条已拍板 + 7 条不做 + 4 条待拍板)** |
| §13 关键参考 | Python + Rust 文件指针 | §11 + 附录 B |

---

## §0 背景与设计目标

### 0.1 为什么需要 sync

**v0.1 的痛点**: 用户打开 bettercursor 看到 17 条 session, 但**只能在原来产生它的端 resume**:

- Linux CLI (`cursor-agent`) 创建的 session → Mac Electron 看不到 (Layer 2 store.db 不在 Mac)
- Mac Electron 创建的 session → Linux CLI 看不到 (Layer 3 state.vscdb 不在 Linux)
- Linux CLI 与 Linux Electron Desktop 同机 → 互相看不到对方的 session

**用户实际工作流** (基于多轮对话):

1. **Mac 跟 agent 聊** → 中途 SSH 到 Linux → 想在 Linux CLI 接续
2. **Linux CLI 跟 agent 聊** → 回到 Mac → 想在 Mac Sidebar 接续
3. **写完代码** → 切到 Linux 编译 → 跑测试 → agent 应该记得上下文
4. **出门带笔记本** (mobile-style) → 一边写一部分 → 回 host 同步

### 0.2 "好 sync 的 5 条判据"

| 判据 | 含义 |
|------|------|
| **无感** | 用户不需要每次手动按 "Sync Now"; 也不需要 daemon/tailscale/SaaS 配置 |
| **跨端可 resume** | 任意端创建的 session, 在**任意**其他端可见 + 可 `--resume` |
| **用户控制** | 数据在自己机器上; 不上云, 不过 Tailscale mesh (除非用户选择) |
| **可手动备份** | `cp -r ~/.bettercursor/` 就是完整备份; 不需要专用工具 |
| **不锁 Cursor** | sync 写期间不破坏 Cursor / cursor-agent 的正常运作, 不留半截 state |

### 0.3 这份文档的 framing 转向 (跟旧版最关键的差异)

| 维度 | 旧 SYNC_DESIGN (2026-07-03) | 新 SYNC_DESIGN (本文件) |
|------|---------------------------|------------------------|
| 数据形态 | 单一 `~/.bettercursor/unified.db` 在 hub | **每台机器各自一份** unified.db (本机内部缓存) |
| 跨设备传输 | Tailscale mesh + 公网 SSH | **SSH/rsync** (默认), Tailscale 仅作为可选 (T5) |
| Snapshot 编码 | gzip JSON (`*.json.gz`) | **纯文本 JSON** (为 3-way merge) |
| 冲突解决 | LWW only | **LWW + bubble-level diff + 3-way UI**, 写前自动 archive |
| 锁检测 | sync.rs 内联 `cursor_processes_running` | 独立 `core::process`, 升级为分类型 `core::lock` |
| 离线交付 | 本地 sync loop | **离线 outbox 队列** + 上线后 flush |
| 备份 | 未设计 | **`cp -r ~/.bettercursor/`** (用户自管) |

### 0.5 v0.2-alpha + v0.2.1 已落地的能力 (区别于本设计稿的"待做"部分)

> 这两个 milestone 都已合入 main, 是设计稿的**前置约束**. 看后续 §3-§9 时请带着这两栏的现状.

#### v0.2-alpha (2026-07-03) — 手动 L2↔L3 补层 sync

| 能力 | 实现位置 |
|------|---------|
| `core::process::cursor_processes_running()` | `src-tauri/src/core/process.rs` (117 行) |
| `core::sync::sync_session(uuid, cwd) → SyncReport` | `src-tauri/src/core/sync.rs` (~1453 行, 旧 sync 部分) |
| Tauri command `sync_session_layer23` | `src-tauri/src/lib.rs` |
| Frontend `syncSessionLayer23` wrapper | `src/lib/tauri.ts` |
| SessionDetail 顶部 sync banner (三态 idle/running/done) | `src/components/SessionDetail.tsx` |

**关键设计约束** (写后续 v0.3+ 模块时必须兼容):
- **单 session inline 写**: 一次按钮触发, 单 session 补层, **不**走 outbox, **不**走 daemon. (用户拍板: "不要在应用内生成脚本".)
- **补层 (单向补齐) 语义**: 只补缺失的层; 不合并内容. 不动已存在的 blob.
- **硬锁策略**: `super::process::cursor_processes_running()` 命中 → `skipped=["cursor_running(...)"]` 拒绝.
- **写前备份**: 写前 `.backup_<ts>`, 写后 `PRAGMA integrity_check` + `wal_checkpoint(TRUNCATE)`.
- **L2 root 修复**: `fix_latest_root` (sync.rs:612) 把 `meta[0].latestRootBlobId` 写回, 让 `--resume` 工作.

**遗留 / 已知限制**:
- L2 写入路径退化: 当 Layer 3 conversationState base64 解析失败时, 退化到从 Layer 1 JSONL 合成 bubble blobs; tool_use 结构丢失 (因为 Layer 1 没有).
- L3 写入路径不读 Layer 2 conversationState: 气泡按 `inject.rs::compose_bubble_blobs` 模板填, **是"让 Sidebar 看见"而不是"完美恢复对话"**.

#### v0.2.1 (2026-07-04) — 修 orphan + 删除 session

| 能力 | 实现位置 |
|------|---------|
| `core::sync::fix_orphans → FixOrphansReport` | `src-tauri/src/core/sync.rs:651-720` |
| `core::sync::delete_session → DeleteReport` | `src-tauri/src/core/sync.rs:763-870` |
| Tauri commands `fix_orphans` + `delete_session` | `src-tauri/src/lib.rs` |
| UI: SessionTree "Wrench" 批量按钮 + 4s toast | `src/components/SessionDetail.tsx` |
| UI: SessionDetail "修复 Layer 2" 单按钮 (broken 时显示) | `src/components/SessionDetail.tsx` |
| UI: SessionDetail "删除" + 原生 `<dialog>` 确认 (L1/L2 checkbox + L3 disabled) | `src/components/SessionDetail.tsx` |

**v0.2.1 拍板的硬约束** (写后续 §3-§9 时也必须兼容):
- **L3 delete 跳过**: Cursor Desktop 自己管 state.vscdb 与 workspaceStorage 之间的引用, 强制写可能损坏; 删 session 只删 L1 + L2.
- **delete = `remove_dir_all`**: 没有 trash sidecar, 不走 staging; 用户明示"直接 rm". 前置 `cursor_processes_running()` 锁.
- **`fix_orphans` 全量扫**: 不区分单条/批量, 一个后端函数, 前端 SessionDetail 单条入口 + SessionTree 全量入口都调它. 写之前自动 `.backup_<ts>` 兄弟文件.
- **锁检测模块独立**: `core::process` 提取出来, 测试覆盖; sync.rs 调用 `super::process::cursor_processes_running()`.

#### v0.2.x 之后**还没有**的实现 (本设计稿的范围)

| 能力 | 本设计稿 |
|------|---------|
| `~/.bettercursor/unified.db` (本机 SQLite 缓存) | §3 |
| Snapshot codec (跨端纯文本 JSON) | §2 |
| Transport adapters (T0-T5) | §4 |
| 冲突检测 (5-way) + 3-way merge UI | §6 |
| `core::lock` 升级 (vs `core::process` 单点查询) | §7 |
| 离线 outbox 队列 | §5 |
| `cp -r ~/.bettercursor/` 自管备份 UX | §5 |

---

## §1 核心架构

### 1.1 顶层框图

```
┌─────────────────────────────────────────────────────────────────────┐
│  本机: Mac / Linux (bettercursor Tauri 桌面应用)                       │
│                                                                     │
│  ┌──────────────────┐  ┌────────────────────────────────────────┐  │
│  │  React Frontend  │  │  Rust Backend (Tauri commands)          │  │
│  │  (WebView)       │←→│                                        │  │
│  │  SessionTree     │  │  core::paths / storage / canonical  ✦   │  │
│  │  SessionDetail   │  │  core::process      (v0.2-alpha ✅)     │  │
│  │  SyncBanner ✦    │  │  core::sync         (v0.2.1 ✅ inline)  │  │
│  │  WrenchButton ✦  │  │  core::inject       (Mutation)         │  │
│  │                  │  │  ────────────────────────────────       │  │
│  │                  │  │  core::snapshot    (NEW, §2)            │  │
│  │                  │  │  core::unified      (NEW, §3)            │  │
│  │                  │  │  core::transport    (NEW, §4)            │  │
│  │                  │  │  core::conflict     (NEW, §6)            │  │
│  │                  │  │  core::lock         (NEW, §7, ←process)  │  │
│  │                  │  │  cli::bettercursor  (NEW binary, §5)     │  │
│  └──────────────────┘  └─────────┬──────────────────────────────┘  │
│                                 │                                   │
│                                 ↓ (R/W + WAL-safe temp copy)         │
│  ┌────────────────────────────────────────────────────────────┐    │
│  │  ~/.bettercursor/   (本机; "internal cache, not canonical") │    │
│  │  ├─ unified.db      SQLite WAL + busy_timeout=5s (§3)      │    │
│  │  ├─ snapshots/<host>/*.json   纯文本 JSON (§2)             │    │
│  │  ├─ archive/<uuid>/<ts>.json  冲突前自动 archive (§6)       │    │
│  │  └─ outbox/<host>/*.json      离线队列 (§5)                 │    │
│  └────────────────────────────────────────────────────────────┘    │
│                                 │                                   │
│                                 ↓ (Transport adapter, §4)            │
│  ┌────────────────────────────────────────────────────────────┐    │
│  │  Cursor 三层 (CANONICAL)                                     │    │
│  │  • Layer 1: JSONL (transcript; both CLI + Desktop write)    │    │
│  │  • Layer 2: store.db (SQLite WAL, single-writer=cursor-agent)│    │
│  │  • Layer 3: state.vscdb (SQLite WAL, multi-writer + cursor- │    │
│  │              server long-linger)                            │    │
│  └────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────┘
                              ↕ (only v0.3+)
                              ↕ via T2 SSH/rsync (default; §5)
┌─────────────────────────────────────────────────────────────────────┐
│  远端: 另一台 Mac / Linux (同样的 bettercursor 安装)                   │
│  (相同的 ~/.bettercursor/ 布局; 各持有自己的 unified.db)              │
└─────────────────────────────────────────────────────────────────────┘
```

✦ = v0.2-alpha / v0.2.1 已实现的 UI 组件

### 1.2 关键概念 framing

#### Unified.db 是**本机内部缓存**, 不是 canonical store

`~/.bettercursor/unified.db` 是从 **Cursor 三层 (Layer 1 JSONL / Layer 2 store.db / Layer 3 state.vscdb)** 派生出来的 SQLite 数据库. 它:

- 给 UI 提供快速查询 (FTS5 全文搜索, 见 §3)
- 记录 sync 历史 (`sync_runs` 表)
- 暂存被覆盖前的快照 (`archive` 表)
- 记录冲突标记 (`conflicts` 表)

但它**不是**真相来源. 真相永远是 Cursor 自己的三层. 这意味着:

- **不可写**: 写只能写 Cursor 三层 (通过 `core::inject::Mutation` / `core::sync::sync_session` 现有路径).
- **可重建**: unified.db 损坏就 `cargo tauri dev` 启动时 `core::canonical::scan_all()` 重新回填.
- **不同步**: 我们 sync **Cursor 三层的内容** (序列化进 snapshot), 不是同步两份 unified.db.

#### Snapshot 是**跨端数据交换的中间表示**

跟 unified.db 不同, snapshot 是**只读的、按 session 粒度的、纯文本的**. 任何端 → 任何端的传输都用 snapshot 作为 unit. (见 §2)

#### Transport 是**适配器模式**, 不是单一通道

我们不锁定 SSH/rsync 也不锁定 Tailscale. `core::transport::Transport` trait (见 §4) 有 6 层 tier (T0–T5), 默认是 T2 SSH/rsync. 用户可以选 T5 Tailscale (自动感知的 SSH), 也可以退到 T0 手动 (USB/NAS 拷).

### 1.3 模块清单

| 模块 | 状态 | 职责 |
|------|------|------|
| `core::paths` | ✅ v0.1 | 4 层路径解析 (Mac/Linux/Windows) |
| `core::storage` | ✅ v0.1 | WAL-safe SQLite 读 |
| `core::canonical` | ✅ v0.1 | 扫三层 + 合并 (`sync_all` 回填 unified.db) |
| `core::process` | ✅ v0.2-alpha | `cursor_processes_running()` (5 PATTERNS + self-filter) |
| `core::sync` | ✅ v0.2.1 | `sync_session` / `fix_orphans` / `delete_session` (inline-write) |
| `core::inject` | ✅ v0.2-alpha | `Mutation` enum + `compose_*` (bubble blobs) |
| **`core::snapshot`** | 🆕 NEW | `SessionSnapshot` / `Bubble` 纯文本 JSON 编解码 (§2) |
| **`core::unified`** | 🆕 NEW | `UnifiedDb` SQLite 7 表 CRUD (§3) |
| **`core::transport`** | 🆕 NEW | `Transport` trait + 6 层 tier 实现 (§4, §5) |
| **`core::conflict`** | 🆕 NEW | 5-way 分类 + bubble-diff (§6) |
| **`core::lock`** | 🆕 NEW | 升级自 `process`: 分类型 + idle 检测 + SIGTERM (§7) |
| **`cli::bettercursor`** | 🆕 NEW binary | `bettercursor push --to=linux` CLI (§5) |

---

## §2 Snapshot Codec

### 2.1 设计目标

Snapshot 是 sync 系统的**通用数据交换格式**. 任何端 → 任何端, 都先把本地 session 序列化成 Snapshot, 再传到对面. 关键属性:

- **纯文本 JSON** (不 gzip). 理由见下.
- **self-contained**: 每份 snapshot 包含该 session 的全部内容 (bubbles + blobs), 不需要外部 ref.
- **按 session 粒度**: 一个 snapshot = 一个 composer_id.
- **sha256 命名**: 文件名带内容 hash, 避免重复 import.
- **来源带端点**: `source_endpoint` 字段携带 host/os/user, 用于追溯.

### 2.2 为什么**不**用 gzip (对比旧版)

旧版 (`SYNC_DESIGN.md §3.2` of 2026-07-03) 用 `gzip` 压缩. 这跟新版矛盾. 理由:

| 维度 | gzip JSON | 纯文本 JSON |
|------|----------|------------|
| 磁盘占用 | 小 (3-5 MB/session) | 大 (10-15 MB/session) |
| git 友好 | ❌ 二进制, diff 没用 | ✅ 文本 diff 可读 |
| 3-way merge | ❌ 需先解压, 再 merge, 再压 | ✅ 直接 `git merge-file` |
| partial recovery | ❌ 损坏整文件 | ✅ 损坏部分行也可读 |
| rsync 友好 | ⚠️ 整体传 | ✅ rsync --partial 增量传 |

我们 sync 的本质是 "**让两端会话内容一致**". 偶尔磁盘多用 10 MB 是值得的: 用户可以 `git log` 看历史, 出问题手动 `vim` 修一行就恢复.

### 2.3 字节布局

```
~/.bettercursor/snapshots/<host>/<uuid>-<ts>.json   ← 文件路径, host = 此份 snapshot 来源
├─ {
├─   "version": 4,                          ← schema 版本 (旧 v3 gzip → 新 v4 plain text)
├─   "exported_at": 1719... (ms epoch),
├─   "source_endpoint": {
├─     "host": "macbook-pro-m1",
├─     "os": "macos",
├─     "user": "eric",
├─     "endpoint_kind": "mac",
├─     "cursor_version": "1.2.4"
├─   },
├─   "composer": {
├─     "composer_id": "465b0684-...",      ← UUID
├─     "last_updated_at": 1719...,        ← LWW 主键
├─     "project_path": "/Users/eric/...",  ← 来自 L3 workspaceIdentifier.uri.fsPath
├─     "project_slug": "enenzuo",         ← 来自 L1 dir 名 (见 §2.5)
├─     "workspace_id": "b9c96f34...32ch", ← 来自 L3 workspaceIdentifier.id; 或 "empty-window"
├─     "chat_root": "md5(cwd) hex"        ← Layer 2 目录 basename
├─   },
├─   "bubbles": [                          ← 关键决定: bubble-level 不是 blob-level
├─     {
├─       "id": "...",                      ← bubble UUID (从 L3 bubbleId:<uuid>:<bid> 取)
├─       "role": "user" | "assistant",
├─       "text": "...",
├─       "tool_calls": [{ "name": "...", "input": {...} }],
├─       "files": ["..."],
├─       "ts": 1719...,                     ← ms epoch
├─       "parent_bubble_id": "..." | null
├─     },
├─     ...                                  ← 100-300 个 bubble 正常
├─   ],
├─   "blob_refs": ["sha256hex...", ...],    ← 该 session 引用的 blob 哈希
├─   "raw_blobs": {                        ← 仅当前端尚未在其他位置 materialize 时填
├─     "sha256hex": "<base64>",
├─     ...
├─   }
├─ }
```

### 2.4 为什么是 **bubble-level** 不是 blob-level (对比 cursaves v3)

旧版 (`sync.rs:82-153` 已经用了 cursor-agent 的 store.db 的 protobuf blob DAG) 是 blob-level. 但这跟"用户视角看对话"不匹配:

- blob 是 Cursor 内部优化 (相同文本 chunk 复用). 不是一个稳定不变的 unit.
- bubble 是用户视角的"一条消息", 才是会话的 content unit.
- bubble 跟 blob 不是 1-to-1: 一个 bubble 可能拆成 3 个 blob (text + tool_call + files).

新版 snapshot **不再依赖 store.db 的 blob DAG**, 直接从 §9 的 inline-write 路径读 bubble list. 这意味着 L2 写入路径需要重新生成 blob DAG, 但反正 v0.2-alpha 已经做了 (`core::sync::write_layer2` + `fix_latest_root`).

### 2.5 Cursor 数据 quirk (snapshot 编码时必须处理的 5 个事实)

> 这 5 条**不是设计决策**, 是用户机器实测抽取的 Cursor 存储 quirk. snapshot 编码 / unified.db 字段 / 冲突分类都必须 reflect 这些 quirk. 见原 SYNC_DESIGN §3.4 (2026-07-03 → 2026-07-04).

#### Q1. Layer 1 (JSONL) **不是** origin 标识 (#87/#88)

Layer 1 JSONL 由 Cursor Desktop Electron 和 cursor-agent CLI **都会写**. 不能用 "Layer 1 文件存在 → 标记 `linux_cli`". 正确做法:
- **Layer 1**: 只填 `first_user_message_preview` / `indexable_text` / `project_slug` — 这些是 Layer 1 独有的 metadata
- **Layer 2 (store.db)**: CLI 专属, 是 `linux_cli` 来源的**唯一可信标记**
- **Layer 3 (composerData:<uuid>)**: Desktop 专属, 是 `linux_desktop` 来源的**唯一可信标记**

`scan_layer1_into` 必须**不调用** `merge_source(SourceLayer::LinuxCli, ...)`. 这条规则在 #88 之前被违反过, 导致 Desktop-only session 被错误打上 `linux_cli` 标签.

#### Q2. Layer 3 `workspaceIdentifier.id = "empty-window"` (Cursor 特殊常量)

Cursor Desktop 在空 window 模式下创建会话时:

```json
{"id": "empty-window"}
```

`id` 是字符串字面量 `"empty-window"`, 而不是 workspaceStorage 目录的 32-char hash. `uri.fsPath` 为空字符串.

`extract_workspace_path` 必须同时判断 `id == "empty-window"` 和 `uri.fsPath` 为空, 才走 "no workspace" fallback. 否则会被误识别为有 workspace.

**实测** (用户机器): 43 条 Layer 3 composer 中, 29 条 `workspaceIdentifier.id == "empty-window"` (真正的空 window 创建), 14 条 `id` 是 32-char hex 但对应 workspaceStorage 目录已被 Cursor 清理 (用户删过 folder).

#### Q3. Layer 2 `chat_root = md5(cwd)` 反查 (#100)

Layer 2 chat_root 是 md5(cwd) — 不可逆哈希. 反查策略:

1. **优先** `~/.config/Cursor/User/workspaceStorage/<hash>/workspace.json`: 读 `folder` URI, md5 该路径, 匹配 → 用该 hash 作为 slug.
2. **回退** `~/.cursor/projects/<slug>/agent-transcripts/`: 该路径**不可靠** — Cursor 的项目 slug (`home-eric-workspace-enenzuo`) 是 sanitize(cwd) 不是 md5, 同一 slug 下可有多个 chat_root. **不要**用它做反查.
3. **最终 fallback**: 保留 `chat-<md5>` 作为唯一 slug. (用户视角下仍是孤立的 hash 字符串, 但不会跟真项目合并.)

#### Q4. Cursor 残留的"空 chat_root / 数字 slug" 目录

用户机器上存在 `~/.cursor/chats/<md5>/` 完全空的目录 (Cursor 创建了 bucket 但里面的 session 早就清空), 以及 `~/.cursor/projects/<数字时间戳>/` (例如 `1778988228984`) — Cursor 老版本或临时模式生成的残余 bucket. **不要**为它们建 session; `scan_layer2_into` / `scan_layer1_into` 通过"目录下没有任何 store.db / JSONL" 自然跳过, 不需要特殊处理.

#### Q5. 项目分组的 slug 收敛策略 (#99)

用户视角下, 真项目的 session 应该归到一个可读的分组; 孤儿的 session 应该归到同一个 `no-workspace` 分组.

| 数据来源 | project_slug 取值 | UI 标题示例 |
|----------|------------------|------------|
| L3 有 workspaceIdentifier + L1/L2 命中真项目 | `home-eric-workspace-enenzuo` (L1 dir 名) | enenzuo (10) |
| L2 命中, workspaceStorage 反查命中 | workspaceStorage hash (32 char) | b9c96f3499915796... (4) |
| L2 命中, workspaceStorage 反查失败 | `chat-<md5>` | chat-592ffb8da5... (1) |
| L3 有 `empty-window` 或 fsPath 为空 | `"no-workspace"` | no-workspace (43) |
| 老的 `desktop-<uuid>` fallback (已废弃) | `"no-workspace"` (新值) | no-workspace |

**旧行为 bug**: `desktop-<uuid>` 让人误以为这是 Cursor 命名的真项目. 已统一改为 `"no-workspace"`.

### 2.6 Rust signatures

```rust
// ── snapshot.rs ──────────────────────────────────────────

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Snapshot schema version. v4 = "plain text JSON, bubble-level"
/// (vs v3 gzip blob-level from cursaves).
pub const SNAPSHOT_VERSION: u32 = 4;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SessionSnapshot {
    pub version: u32,
    pub exported_at: i64,                // ms epoch
    pub source_endpoint: SourceEndpoint,
    pub composer: ComposerMeta,
    pub bubbles: Vec<Bubble>,
    pub blob_refs: Vec<String>,         // sha256 hex
    pub raw_blobs: std::collections::HashMap<String, String>,  // sha256 → base64
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SourceEndpoint {
    pub host: String,                   // hostname
    pub os: String,                     // "macos" | "linux" | "windows"
    pub user: String,                   // $USER
    pub endpoint_kind: String,          // "linux_cli" | "mac" | "linux_desktop"
    pub cursor_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ComposerMeta {
    pub composer_id: String,            // UUID
    pub last_updated_at: i64,           // ms epoch, LWW 主键 (§6)
    pub project_path: String,           // L3 workspaceIdentifier.uri.fsPath, 或 ""
    pub project_slug: String,           // 见 §2.5 Q5
    pub workspace_id: String,           // L3 workspaceIdentifier.id (32 char hex) 或 "empty-window"
    pub chat_root: String,              // Layer 2 dir basename = md5(cwd)
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Bubble {
    pub id: String,                     // bubble UUID
    pub role: String,                   // "user" | "assistant"
    pub text: String,
    pub tool_calls: Vec<ToolUse>,
    pub files: Vec<String>,
    pub ts: i64,                        // ms epoch
    pub parent_bubble_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolUse {
    pub name: String,
    pub input: serde_json::Value,
}

/// Serialize to plain text JSON (NOT gzipped). Caller writes to
/// ~/.bettercursor/snapshots/<host>/<uuid>-<ts>.json.
///
/// `host = source_endpoint.host` (snapshot 来源端), so we get a
/// namespaced dir per remote.
pub fn encode_snapshot(s: &SessionSnapshot) -> String;

/// Parse a snapshot file. Returns Err on schema version mismatch
/// (caller decides whether to upgrade from v3 → v4 with a separate
/// `upgrade_from_v3_gzip` shim — TBD).
pub fn decode_snapshot(json: &str) -> anyhow::Result<SessionSnapshot>;

/// Write snapshot file with sha256-named sibling. Atomically renames
/// from a tmp dir so a partial write can't be observed.
pub fn write_snapshot_file(
    out_dir: &Path,
    host: &str,
    snap: &SessionSnapshot,
) -> anyhow::Result<std::path::PathBuf>;
```

### 2.7 跟上游参考的关系

| 参考源 | 格式 | 与 v4 codec 关系 | 可借鉴 |
|--------|------|-----------------|--------|
| [`bettercursor/snapshot.py`](bettercursor/snapshot.py) | v3 gzip blob-level | **不兼容** v4 | 历史对照 only |
| [`vendored/cursaves/cursor_saves/export.py`](../vendored/cursaves/cursor_saves/export.py) | v3 gzip + agentBlobs + messageContexts + checkpoints | **不采用** v3 作主格式 (见 §2.4) | ancillary 字段清单、`_extract_agent_blob_ids`、`_trim_message_contexts` |
| [`vendored/cursaves/cursor_saves/importer.py`](../vendored/cursaves/cursor_saves/importer.py) | 导入 + 冲突五态 + workspace 注册 | v4 import 路径参考 | `_check_conflict`、agent_blobs 写入、`repair_missing_blobs` |
| [`vendored/cursor-history/specs/`](vendored/cursor-history/specs/) | bubble-level 消息模型 | **语义对齐** v4 `bubbles[]` | spec 010 timestamp / 012 完整性 / 013 tool 不截断 |

> **不要**从 v3 gzip → v4 plain text 自动 upgrade — 让用户手动 export/import 一次, 显式选择 codec 版本.
>
> cursaves v3 gzip 与 bettercursor v4 plain text 是**有意分叉**; 借鉴的是字段清单与算法, 不是文件格式.

### 2.8 L3 bubble 解析深度缺口 (cursor-history 对标)

> v0.2.2 三路合并已落地, 但 L3 读路径仍过浅: `decode_l3_bubble_blob` 基本只取 `text` 字段. Desktop agent 气泡大量内容在 `toolFormerData` / `thinking.text` / `codeBlocks[]`, 导致对话展开常显示空或残缺 assistant 气泡.

**目标**: 在 `canonical.rs` 新增 `extract_l3_bubble_text()` + 结构化 `tool_calls`, 协议变更同步 `src/lib/types.ts`.

| 能力 | cursor-history 参考 | bettercursor 落点 | 优先级 |
|------|-------------------|------------------|--------|
| `toolFormerData` → 可读文本 + `ToolCall` | `storage.ts::extractBubbleText` / `extractToolCalls` | `canonical.rs::decode_l3_bubble_blob` | **高** |
| `read_file_v2` / `edit_file_v2` / terminal output 存储层不截断 | spec 013 | 同上; UI 层再做 preview 截断 | **高** |
| Timestamp 多级 fallback + `fillTimestampGaps` | spec 010, `storage.ts::extractTimestamp` | `decode_l3_bubble_blob` + `merge_bubbles_three_way` 排序 | **高** |
| Token / model / session usage | spec 009 | `Bubble` 扩展 → `unified.rs` ingest | 中 |
| 降级标记 `source: workspace-fallback` / `metadata.corrupted` | spec 012 | `Conversation` + UI 警告条 | 中 |
| Message type 过滤 (user/assistant/tool/thinking) | spec 008 | `MessageList` 过滤器 (依赖上表 tool 识别) | 低 |

**反模式** (spec 013 FR-008): Display 层截断逻辑**不得**下沉到 Rust 存储/提取层 — 会损害 unified.db FTS 与 v4 codec 完整性.

---

## §3 unified.db Schema

### 3.1 定位

`~/.bettercursor/unified.db` 是**本机 SQLite 缓存**, 不是 canonical store. 它的目的是:

| 用途 | 来源 |
|------|------|
| FTS5 全文搜索 (`bubble.text`) | 从 Cursor 三层回填 |
| 多源合并 (L1 + L2 + L3 同一 uuid) | `core::canonical::scan_all` 派生 |
| Sync 历史记录 (`sync_runs`) | `core::sync` 写入 |
| 被覆盖前的 snapshot (`archive`) | 冲突解决前自动 archive (§6) |
| 冲突标记 (`conflicts`) | 5-way 分类持久化 (§6) |

### 3.2 设计约束

| 约束 | 理由 |
|------|------|
| SQLite **WAL 模式** | 多读单写, Cursor 自己用 WAL, 我们跟着用 |
| **busy_timeout = 5000ms** | 等 5 秒避开 Cursor 短促的写 |
| **没有触发器** | 触发器调试困难, unified.db 是 derived cache, 损坏就 `scan_all` 回填 |
| **FTS5 on bubbles.text** | 用户视角搜索关键字最自然 |
| **启动时全量回填** | 比 update 触发器更简单可靠; 启动耗时 <1s |
| **不加加密** | 用户自管, 加密交给磁盘加密 (FileVault / LUKS) |

### 3.3 ER 图

```
┌──────────┐      ┌─────────────┐
│ sessions │ ─1:N─│   bubbles   │ ← FTS5 virtual table on text
└──────────┘      └─────────────┘
     │ 1
     │
     ├─1:N─→ blobs  ← sha256 → base64
     ├─1:1─→ composer_data ← L3 完整 JSON (state 来源)
     ├─1:N─→ sync_runs  ← 每次 sync 尝试一行
     ├─1:N─→ archive    ← 冲突前存的 snapshot 引用
     └─1:N─→ conflicts  ← 5-way 分类标记
```

### 3.4 Schema (7 张表 + 1 索引)

```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
PRAGMA foreign_keys = ON;

CREATE TABLE sessions (
    uuid TEXT PRIMARY KEY,                    -- composer_id
    project_slug TEXT NOT NULL,               -- 见 §2.5 Q5
    project_path TEXT,                        -- L3 workspaceIdentifier.uri.fsPath; or ""
    workspace_id TEXT,                        -- L3 workspaceIdentifier.id, or "empty-window"
    chat_root TEXT NOT NULL,                  -- md5(cwd) hex
    origin_kind TEXT NOT NULL,                -- "linux_cli" | "mac" | "linux_desktop"
    name TEXT NOT NULL,                       -- title (first user msg preview)
    first_user_message_preview TEXT,
    bubble_count INTEGER NOT NULL DEFAULT 0,
    last_updated_at INTEGER NOT NULL,         -- ms epoch, LWW 主键 (§6)
    sources_json TEXT NOT NULL,               -- JSON: { linux_cli: { layer, path, ... }, ... }
    created_at INTEGER NOT NULL,              -- ms epoch
    updated_at INTEGER NOT NULL               -- ms epoch (last scan_all touched this row)
);
CREATE INDEX idx_sessions_last_updated_at ON sessions(last_updated_at DESC);
CREATE INDEX idx_sessions_project_slug ON sessions(project_slug);

CREATE TABLE bubbles (
    id TEXT PRIMARY KEY,                      -- bubble UUID
    composer_uuid TEXT NOT NULL REFERENCES sessions(uuid) ON DELETE CASCADE,
    role TEXT NOT NULL,                       -- "user" | "assistant"
    text TEXT NOT NULL,                       -- 全文搜索主字段 (FTS5 mirror)
    tool_calls_json TEXT,                     -- JSON: [{name, input}, ...]
    files_json TEXT,                          -- JSON: ["path1", ...]
    ts INTEGER NOT NULL,
    parent_bubble_id TEXT                     -- self-ref or null
);
CREATE INDEX idx_bubbles_composer ON bubbles(composer_uuid, ts);

-- FTS5 mirror on bubbles.text (NOT a separate source — content table)
CREATE VIRTUAL TABLE bubbles_fts USING fts5(
    text, content='bubbles', content_rowid='rowid'
);

CREATE TABLE blobs (
    sha256 TEXT PRIMARY KEY,                  -- hex
    data_b64 TEXT NOT NULL,                   -- base64 of the blob
    size INTEGER NOT NULL,
    first_seen_at INTEGER NOT NULL
);

CREATE TABLE composer_data (
    composer_uuid TEXT PRIMARY KEY REFERENCES sessions(uuid) ON DELETE CASCADE,
    workspace_identifier_json TEXT NOT NULL,  -- L3 workspaceIdentifier
    full_json TEXT NOT NULL,                  -- 整个 composerData:<uuid> 的原文
    updated_at INTEGER NOT NULL
);

CREATE TABLE sync_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    triggered_by TEXT NOT NULL,               -- "user_button" | "daemon_loop" | "outbox_flush"
    scanned INTEGER NOT NULL DEFAULT 0,
    imported INTEGER NOT NULL DEFAULT 0,
    conflicts INTEGER NOT NULL DEFAULT 0,
    skipped INTEGER NOT NULL DEFAULT 0,
    error TEXT
);
CREATE INDEX idx_sync_runs_started_at ON sync_runs(started_at DESC);

CREATE TABLE archive (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    composer_uuid TEXT NOT NULL,
    snapshot_path TEXT NOT NULL,              -- 实际 JSON 文件路径 (§2.3)
    reason TEXT NOT NULL,                     -- "before_overwrite" | "before_delete" | "conflict_resolved"
    archived_at INTEGER NOT NULL
);
CREATE INDEX idx_archive_composer ON archive(composer_uuid, archived_at DESC);

CREATE TABLE conflicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    composer_uuid TEXT NOT NULL,
    detected_at INTEGER NOT NULL,
    kind TEXT NOT NULL,                       -- "Diverged" | "IncomingNewer" | ...
    -- 5-way 分类结果 (§6)
    local_snapshot_path TEXT,
    incoming_snapshot_path TEXT,
    bubble_diff_json TEXT,                    -- JSON: { a_only, b_only, common }
    resolved_at INTEGER,
    resolution TEXT                           -- "lww_local_wins" | "lww_incoming_wins" | "user_merged" | null
);
CREATE INDEX idx_conflicts_unresolved ON conflicts(composer_uuid) WHERE resolved_at IS NULL;
```

### 3.5 关键 schema 决策表

| 决策 | 选项 | 拍板 |
|------|------|------|
| 表存哪 | `~/.bettercursor/unified.db` | ✅ (snake_case) |
| 全文搜索 | FTS5 (内置) | ✅ (vs 全部 LIKE '%q%') |
| `bubbles` 怎么存 | content table + FTS5 mirror | ✅ (避免双源) |
| 是否用触发器 | 否, 启动时全量回填 | ✅ (vs 增量 update) |
| 并发模式 | WAL + busy_timeout 5s | ✅ |
| 跨端 sync unified.db? | **不** sync, sync 的是 snapshot | ✅ |

### 3.6 Rust signatures

```rust
// ── unified.rs ───────────────────────────────────────────

use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub struct UnifiedDb { conn: Connection }   // owns WAL connection

impl UnifiedDb {
    /// Open or create at `~/.bettercursor/unified.db`. Sets WAL +
    /// busy_timeout = 5000ms automatically.
    pub fn open(path: &Path) -> anyhow::Result<Self>;

    /// Rebuild from `core::canonical::scan_all()`. Deletes all rows
    /// in `sessions`/`bubbles`/`composer_data`, repopulates; `blobs`
    /// is deduped by sha256 (insert-ignore); `sync_runs`/`archive`/
    /// `conflicts` are kept (history).
    pub fn rebuild_from_cursor_state(&mut self) -> anyhow::Result<usize>;

    /// Single-session upsert. Bubbles are diffed: insert new,
    /// update existing (by id), no-op for unchanged.
    pub fn upsert_session(&mut self, s: &SessionMeta, bubbles: &[Bubble]) -> anyhow::Result<()>;

    /// Search via FTS5 on bubbles.text. Returns matching bubbles
    /// with their composer_uuid.
    pub fn search_bubbles(&self, q: &str, limit: u32) -> anyhow::Result<Vec<BubbleHit>>;

    /// Record a sync attempt.
    pub fn record_sync_run(&mut self, run: &SyncRunRecord) -> anyhow::Result<i64>;

    /// Archive a snapshot file before overwrite (§6).
    pub fn record_archive(
        &mut self,
        composer_uuid: &str,
        snapshot_path: &Path,
        reason: &str,
    ) -> anyhow::Result<i64>;

    /// Find unresolved conflicts (WHERE resolved_at IS NULL).
    pub fn unresolved_conflicts(&self) -> anyhow::Result<Vec<ConflictRecord>>;
}

/// One row of `sessions` table.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub uuid: String,
    pub project_slug: String,
    pub project_path: String,
    pub workspace_id: String,
    pub chat_root: String,
    pub origin_kind: String,
    pub name: String,
    pub first_user_message_preview: String,
    pub last_updated_at: i64,
    pub created_at: i64,
}

#[derive(Debug)]
pub struct BubbleHit {
    pub bubble_id: String,
    pub composer_uuid: String,
    pub text: String,
    pub ts: i64,
}
```

### 3.7 unified.db 是 cache, 不是 canonical — 边界

| 写权限 | 通过谁 | 备注 |
|--------|-------|------|
| `sessions` 写 | `core::sync` + `core::canonical::scan_all` (回填) | 其他模块不直接写 |
| `bubbles` 写 | `upsert_session` | FTS5 mirror 跟着 content table 走 |
| `composer_data` 写 | `upsert_session` (从 L3 取) | |
| `sync_runs` 写 | `record_sync_run` (sync 起点和终点) | |
| `archive` 写 | `record_archive` (在 §6 三种场景) | 必须在 §6 决策**之前**写 |
| `conflicts` 写 | `record_conflict` (冲突检测) | |
| `blobs` 写 | `core::sync::write_layer2` 派生 (dedupe by sha256) | |

**不允许外部直接 INSERT**. 所有写都通过 `UnifiedDb` 的方法, 保证 FTS5 mirror 同步更新.

---

## §4 传输适配器层

### 4.1 为什么是适配器, 不是单一通道

旧版 (`SYNC_DESIGN.md §6.2`, 2026-07-03) 默认 **Tailscale mesh**, 这要求用户装 Tailscale, 不友好. 新版拆成 **6 层 tier + T2 子层**, **默认走 T2a (LAN TCP + mDNS 配对)**, T2b (SSH/rsync) 降为高级/headless 模式; T5 (Tailscale) 作为跨网升级路径, T0 (手动文件) 作为最低 fallback.

### 4.2 6 层 tier ASCII 栈

```
           ↓ 简单 ↑              auto       ←── 版本演进方向
T5   Tailscale / SSH-direct       (auto, both-end daemon)
T4   S3 / R2 / B2                (auto, cloud)
T3   Git over SSH/HTTPS          (auto, versioned)
T2a  LAN TCP + mDNS + 配对  ⭐ default (同网开箱即用)
T2b  SSH + rsync                 (高级 / headless, 需 transports.json)
T1   Folder watcher              (semi-auto, USB / NAS)
T0   Manual file copy            (manual, fallback)
           ↓ 灵活 ↓              manual     ←── 用户控制方向
```

### 4.3 各 tier 详解

| Tier | 传输方式 | Target user | 优点 | 缺点 | 路线图 |
|------|---------|------------|------|------|--------|
| T0 | 手动 `scp` / U 盘 / AirDrop | 完全离线 / 临时文件 | 零配置 | 需手动, 容易忘 | ✅ 已能跑 (snapshot 文件本来就在 `~/.bettercursor/snapshots/`) |
| T1 | 文件夹 watcher (`core::watcher`) | U 盘 / NAS mount | 半自动 | mount point 要固定 | 待 v0.3.2 (?) |
| **T2a** ⭐ | LAN TCP + mDNS (`_bettercursor._tcp`) + 6 位配对码 | 同网 Mac↔Linux 默认用户 | **零配置发现, 一次配对, 托盘常驻** | 仅限局域网; 无 TLS (v0.3.1 用 pairing secret) | **✅ v0.3.1 默认** |
| **T2b** | SSH + rsync | 已有 SSH key / headless | 零第三方, 增量, 可恢复 | 需手写 `transports.json` + key | ✅ 高级模式 (v0.2.6 起) |
| T3 | Git over SSH/HTTPS | 想看 history 的用户 | diff 可视化, 老 snapshot 可回放 | 仓库膨胀, 写冲突 | 待 v0.3.2 (?) |
| T4 | S3 / R2 / B2 | 多端 ≥3 / 没 SSH | scale, durability | 第三依赖, 1 个 API key 要管 | 待拍板 |
| T5 | Tailscale mesh | 已有 Tailscale 的用户 | 自动发现, 直连 | 需装 Tailscale | 待 v0.3.x |

### 4.4 Transport Trait

```rust
// ── transport/mod.rs ────────────────────────────────────

use async_trait::async_trait;
use std::path::Path;

#[async_trait]
pub trait Transport: Send + Sync {
    /// Push one snapshot to remote storage. Implementations may
    /// dedupe by sha256, write to a tmp dir first, etc.
    async fn push(&self, snap: &SessionSnapshot) -> anyhow::Result<()>;

    /// Pull all snapshots newer than `since` (ms epoch). Returned
    /// order: increasing `exported_at` so caller can apply FIFO
    /// when conflicts resolve in LWW (oldest first).
    async fn pull(&self, since: i64) -> anyhow::Result<Vec<SessionSnapshot>>;

    /// List remote session metadata (without bubbles/blobs payload).
    /// Used by UI to render a "remote session table" in the picker.
    async fn list_remote(&self) -> anyhow::Result<Vec<RemoteSessionMeta>>;

    /// Identifier for logs / UI: e.g. "ssh:macbook" / "s3:bucket-name".
    fn endpoint_id(&self) -> &str;
}

#[derive(Debug, serde::Serialize)]
pub struct RemoteSessionMeta {
    pub uuid: String,
    pub host: String,
    pub last_updated_at: i64,
    pub project_slug: String,
}
```

### 4.5 拍板: T2a LAN 是 default, T2b SSH 是高级, T5 Tailscale 是跨网候选

> 拍板时间: 2026-07-05 (v0.3.1 产品转向, 取代 2026-07-03 的「T2 SSH 默认」).
>
> **理由**:
> - 同网接力应对齐剪贴板/KDE Connect 心智: **装一次、自动发现、配对一次**.
> - mDNS `_bettercursor._tcp` + `trusted_peers.json` 对用户隐藏 JSON 配置.
> - SSH/rsync **保留不删** (`SshRsyncTransport`), 供 headless / 已有 key 的高级用户; 跨网官方推荐 T5 Tailscale.
>
> **实现落点** (v0.3.1):
> - `core::transport::lan` — `BC/1` 协议 (PAIR / PUSH / PUSH v4 body / PULL)
> - `core::discovery` — mDNS 广播与浏览
> - `~/.bettercursor/trusted_peers.json` — 配对结果
> - `~/.bettercursor/outbox/<peer_id>/` — 离线排队 + 5min 后台 flush (`core::sync_loop`)

---

## §5 SSH/rsync Transport (T2b 高级模式)

### 5.1 数据流 ASCII

```
[本机 A: ~/.bettercursor/]                          [本机 B: ~/.bettercursor/]
  ├─ unified.db                                        ├─ unified.db
  ├─ snapshots/<B-host>/*.json     ←── rsync ──→     ├─ snapshots/<B-host>/*.json
  ├─ snapshots/<A-host>/*.json     ──→ rsync ──→     ├─ snapshots/<A-host>/*.json
  ├─ outbox/<B-host>/*.json        ──→ ssh remote 'bettercursor apply' → ┘
  └─ archive/<uuid>/<ts>.json                          └─ archive/<uuid>/<ts>.json
```

**核心不变量**:
- **pull always safe**: 拉对方 snapshot 是只读对自己 (写到 `snapshots/<B-host>/`), 不破坏 B-host 的数据.
- **push needs outbox or lock-clear**: 推到对方意味着对方要写自己的 unified.db 或者对方 session 的 L2/L3. 当对方机器**离线**或对方**Cursor 持有 L3 锁**, push 走 outbox.

### 5.2 离线 outbox 时序 (5 步)

```
时间 ─→
                        本机 A (push)                本机 B
                        ──────────                   ──────────
[1] 用户在 A 端修改 session S
    ↓
    A: 写 outbox/<B-host>/S-<ts>.json

[2] A 探活 B 失败 (SSH connection refused / timeout 3s)
    ↓
    A: outbox 文件保留, 排进下次探活窗口
    ... (B 仍然离线)
    ... (A 用户继续操作, 增量都进 outbox)

[3] 5min 后, A 探活 B 成功 (B 重新上线)
    ↓
    A: ssh B 'bettercursor apply < outbox/<B-host>/S-<ts>.json'

[4] B 端 bettercursor apply 流程:
    B: 读 snapshot
    B: backup_existing(self_store_db / state_vscdb)        ← v0.2-alpha 的备份
    B: 锁检测 (cursor_processes_running → 0)               ← §7
    B: 比较 last_updated_at, 决定 LWW / bubble-diff / 3-way
    B: 写 Snapshot 应用

[5] B 端返回 ack, A 删 outbox 条目
    A: outbox/<B-host>/S-<ts>.json → mv .processed/<ts>/
```

**outbox 文件结构** (`~/.bettercursor/outbox/<host>/`):

```
outbox/<host>/
├─ <uuid>-<exported_at>.json      ← 待发 snapshot (§2.3 编码)
├─ <uuid>-<exported_at>.json
└─ .processed/
   └─ <uuid>-<exported_at>.json   ← 已成功 ack, 留 7 天后清理
```

### 5.3 `~/.bettercursor/` 目录结构总表

```
~/.bettercursor/
├─ unified.db               SQLite WAL (§3)
├─ unified.db-wal           WAL sidecar (must be backed up together)
├─ unified.db-shm           SHM sidecar
├─ snapshots/
│  ├─ <host-a>/            其他机器的 snapshot
│  │  └─ <uuid>-<ts>.json
│  └─ <host-b>/
├─ archive/
│  └─ <uuid>/              冲突前自动 archive 的 snapshot
│     ├─ <ts1>.json        "before_overwrite"
│     └─ <ts2>.json        "before_delete"
├─ outbox/
│  └─ <host-b>/            离线队列 (见 §5.2)
└─ preferences.json        (未来) 用户偏好 (tier 选择 / outbox TTL / ...)
```

### 5.4 卖点: `cp -r ~/.bettercursor/` 就是备份

> "整个 `~/.bettercursor/` 是 plain JSON + SQLite WAL, 没有加密, 没有 custom binary. `cp -r` 就是完整备份. 任何 tier 都能独立重建."

具体:
- snapshot 是 plain text JSON (§2.2 拍板)
- unified.db 是 SQLite WAL, 可以直接 `.dump` 出来或 `cp` 到另一台机器继续用
- archive 也是 plain text JSON
- **不需要**专用备份工具
- 上传到 iCloud Drive / Google Drive / Syncthing 都能 work (虽然是 sync 工具但因为 plain text 数据, 不会跟原文冲突)
- 加密 (FileVault / LUKS / 7zip) 交给用户现有工具

### 5.5 零第三方原则 (跟 v0.2-alpha 一致)

```
T2 default 不需要:
  ❌ Tailscale daemon
  ❌ S3 bucket / API key
  ❌ Git 远程 (GitHub / GitLab)
  ❌ 任何 SaaS 订阅

只需要:
  ✅ 用户已有 SSH key (ed25519 / RSA)
  ✅ 对方机器 SSH 服务 (sshd) 在跑
```

> 这跟 v0.2-alpha 拍板的"全部在 Rust 进程内执行, 不要在应用内生成脚本"一致 — 不引入外部依赖.

### 5.6 CLI binary: `cli::bettercursor`

**普通用户不需要这个**. CLI 是给"一台机器跑 daemon, 另一台不装 bettercursor"场景的. 用法:

```bash
# 在本机 push 一个 snapshot 到远端 (需要 SSH access)
bettercursor push --to=macbook --uuid=465b0684-...

# 在远端没有 bettercursor 的情况下手动 apply (ssh remote)
ssh macbook 'bettercursor apply < ~/.bettercursor/snapshots/macbook/465b0684-...-ts.json'

# 列出本机 outbox (调试用)
bettercursor outbox list

# 全量重新回填 unified.db (损坏恢复)
bettercursor unified rebuild
```

Rust signatures:

```rust
// ── cli/bettercursor.rs (NEW binary) ──────────────────────

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bettercursor")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    // 显式 ~/.bettercursor/ 路径覆盖 (默认 $HOME/.bettercursor)
    #[arg(long, global = true)]
    data_dir: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read Cursor L1/L2/L3, write unified.db.
    Unified {
        #[command(subcommand)]
        action: UnifiedCmd,    // rebuild / status / search <q>
    },
    /// Sync operations.
    Sync {
        #[command(subcommand)]
        action: SyncCmd,       // push / pull / outbox / apply
    },
    /// Diagnostic: list Cursor 进程 (替代 `pgrep -af cursor`).
    Doctor { #[arg(long)] verbose: bool },
}
```

### 5.7 没有 watcher 的 trigger 模型

> 拍板: **不**用 `notify` crate (旧 SYNC_DESIGN §5.2 的设计). 改用 polling + SSH keepalive 探活, 原因:
> - notify 在跨 fs 上不可靠 (sshfs mount)
> - v0.2-alpha 已经把好 watcher 留给 fs 层面 (`core::watcher`), 加上 sync 的 watcher 是 N×N
> - 5min polling + outbox 重发足够同步

具体:
- 默认 5min 间隔, `core::sync::daemon_loop` 跑 (v0.2.3 待做, 见 §10)
- 用户按 "Sync Now" 立即触发一次
- outbox flush 由 SSH keepalive / 周期性探活窗口触发

---

## §6 冲突检测与解决

### 6.1 为什么需要 5-way 分类 (不是 binary 同/不同)

旧 SYNC_DESIGN §6.4 只列了 4 种 "相同时间戳" 粗略策略. 但用户机器实测发现: LWW 决策不能只看**整体** `last_updated_at`, 因为:

- 一端改了第 5 条 bubble, 时间戳比较大
- 另一端改了第 8 条 bubble, 时间戳也比较大
- 整体时间戳两人交叉 → 单一 LWW 会**默默丢掉一边的修改**

正确做法: **bubble-level LWW** (相同 uuid 单 bubble 比, 整体作为汇总), 加上 5-way 分类.

### 6.2 5-way 分类 (从 `bettercursor/conflict.py:21-28` 引用)

```
classify(local, incoming):
  if not local:                 → New              (import as-is)
  elif not incoming:            — (永远不到这里, 全本地情况不冲突)
  elif content_hash(local) == content_hash(incoming):
                               → Identical        (skip)
  elif incoming.last_updated_at > local.last_updated_at
                            → IncomingNewer      (LWW, archive local)
  elif local.last_updated_at > incoming.last_updated_at
                            → LocalAhead         (incoming stale, skip)
  else (cross-modified or both newer than each other in different bubbles):
                               → Diverged          (3-way merge UI)
```

| 分类 | 触发条件 | 自动动作 |
|------|---------|---------|
| **New** | 本地无, 远端有 | import as-is |
| **Identical** | content hash 相同 | skip |
| **IncomingNewer** | 远端 `last_updated_at` 更新 (整 session 视角) | **archive local → 接受 incoming** (§6.5) |
| **LocalAhead** | 本地 `last_updated_at` 更新 | skip incoming (它是 stale) |
| **Diverged** | bubble-level diff 不为零 (两侧都改过, 但改动在不同 bubble) | **3-way merge UI**, 双方都 archive (§6.4) |

> **Python reference**: [`bettercursor/conflict.py:21-28`](bettercursor/conflict.py). v3.1 那份就是这套, 现 port 到 Rust.

### 6.3 bubble-level diff (SQLite `EXCEPT` 风格)

```sql
-- 本机有但对方没 (本地独有)
SELECT id, text, ts FROM bubbles WHERE composer_uuid = ?
EXCEPT
SELECT id, text, ts FROM bubbles WHERE composer_uuid = ?;
   -- (input 来自对方 snapshot's bubbles[])

-- 对方有但本机没
SELECT id, text, ts FROM bubbles WHERE composer_uuid = ?
EXCEPT
SELECT id, text, ts FROM bubbles WHERE composer_uuid = ?;
   -- (input 来自本机)

-- 共同的 bubble_id 但 text 改了
SELECT id, text, ts FROM local_bubbles
WHERE id IN (SELECT id FROM incoming_bubbles)
  AND text != (SELECT text FROM incoming_bubbles WHERE id = local.id);
```

(实际实现是内存 side, 不一定走 SQL; 见 Rust signature.)

### 6.4 决策树 ASCII

```
                   classify(local, incoming)
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
      New               Identical          (timestamp 比较)
        │                   │                   │
        ▼                   ▼       ┌───────────┴───────────┐
   import as-is          skip      ▼                         ▼
                                   IncomingNewer            LocalAhead
                                       │                         │
                                       ▼                         ▼
                              archive local                skip incoming
                              accept incoming              (it's stale)

                                       Diverged
                                          │
                                          ▼
                                   bubble_diff(a, b)
                                          │
                            ┌─────────────┴─────────────┐
                            ▼                           ▼
                     自动可合并                 需要用户介入
                  (bubble_id 重叠               (a_only 和 b_only
                   + content hash 一致)           均有变更)
                            │                       │
                            ▼                       ▼
                    auto-merge                3-way merge UI
                    (拼合 bubbles[])            ┌────────────────┐
                            │                  │ <IncomingBubble>│
                            ▼                  │   ↕ 选择/编辑     │
                       archive BOTH          │ <LocalBubble>    │
                       (before merge)         └────────────────┘
                                                  │
                                                  ▼
                                            用户点 "Apply"
                                                  │
                                                  ▼
                                            archive BOTH
                                            (apply_before_conflict_resolved)
```

### 6.5 强制: 任何覆盖前自动 archive

> **拍板时间**: 2026-07-03 (跟 §6.4 Diverged 顺序一致).
>
> **铁律**: 即将被覆盖的 session, 必须先复制 snapshot 进 `~/.bettercursor/archive/<uuid>/<ts>.json`, 然后才能应用新 snapshot.
>
> **三种触发场景**:
>
> | 场景 | Reason 字段 | 写之前还要做什么 |
> |------|-------------|------------------|
> | `IncomingNewer` 覆盖本地 | `"before_overwrite"` | 写 archive → 写 Cursor L2/L3 (走 `core::sync::sync_session` 现有路径) |
> | 用户手动删 session (v0.2.1 `delete_session`) | `"before_delete"` | 写 archive → `remove_dir_all` |
> | Diverged 3-way merge apply | `"before_overwrite"` + `"before_conflict_resolved"` | 写 archive 双方 → 写 L2/L3 |
>
> archive 是 plain text JSON, 不会让 unified.db 膨胀; 7 天后自动清理 (`core::unified::record_archive` + 一个 cron).

### 6.6 Rust signatures

```rust
// ── conflict.rs ──────────────────────────────────────────

use crate::core::snapshot::{Bubble, SessionSnapshot};
use crate::core::unified::UnifiedDb;

#[derive(Debug, PartialEq)]
pub enum ConflictClass {
    New,
    Identical,
    IncomingNewer,
    LocalAhead,
    Diverged,
}

/// SHA-256 of canonical bubble field concat (text + tool_calls + files).
/// Independent of ts; used to detect "same content, different timestamp".
pub fn content_hash(bubbles: &[Bubble]) -> String;

pub fn classify(
    local: &LocalSnapshotState,    // locally-known composer summary
    incoming: &SessionSnapshot,    // remote composer summary
) -> ConflictClass;

#[derive(Debug)]
pub struct BubbleDiff {
    pub a_only: Vec<Bubble>,        // local-only bubble ids
    pub b_only: Vec<Bubble>,        // incoming-only bubble ids
    pub common_changed: Vec<(Bubble /* local */, Bubble /* incoming */)>,
}

pub fn bubble_diff(a: &[Bubble], b: &[Bubble]) -> BubbleDiff;

/// Mandatory: write archive entry BEFORE any overwrite. See §6.5.
/// Returns the archive row id.
pub fn auto_archive_before_overwrite(
    db: &UnifiedDb,
    composer_uuid: &str,
    snapshot_path: &std::path::Path,
    reason: &str,                    // "before_overwrite" | "before_delete" | "before_conflict_resolved"
) -> anyhow::Result<i64>;

/// Auto-merge attempt: returns Some(merged) when bubble_diff is
/// "no real conflict" (a_only + b_only are appends, common_changed
/// is empty or all match hash); None when Diverged needs UI.
pub fn auto_merge(a: &[Bubble], b: &[Bubble]) -> Option<Vec<Bubble>>;
```

### 6.7 3-way merge UI (用户视角)

> Diverged 场景下, UI 弹出 `<ConflictResolveDialog>` (待 v0.3.1 拍板):
>
> ```
> ┌──────────────────────────────────────────────────────────────┐
> │  sync: 检测到冲突                                            │
> │  composer: 465b0684-aaf1-...                                 │
> │  本机: 3 分钟前 改 bubble #7 "fix the type error"             │
> │  远端: 5 分钟前 改 bubble #8 "add retry logic"                │
> ├──────────────────────────────────────────────────────────────┤
> │  Bubble #7                                                  │
> │  ○ 远端版本:  "fix the type error..."                        │
> │  ● 本机版本:  "fix the type error... but also update retu..." │
> │  ○ 编辑合并                                                  │
> │                                                              │
> │  Bubble #8                                                  │
> │  ● 远端版本:  "add retry logic..."                            │
> │  ○ 本机版本:  "add retry logic..."                            │
> │  ○ 编辑合并                                                  │
> ├──────────────────────────────────────────────────────────────┤
> │  [Cancel]                                    [Apply Merged]  │
> └──────────────────────────────────────────────────────────────┘
> ```
>
> 默认选中 = 时间戳新的一方 (单一 LWW), 但用户可以编辑文本或者选另一边. 提交时调 `auto_archive_before_overwrite(reason="before_conflict_resolved")`, 然后走 §9 写流程.

---

## §7 L1/L2/L3 锁画像与并发安全

> 这是本设计稿的**最有新颖性**的章节 (旧版没有). 三层 Cursor 存储有三种完全不同的并发语义, 必须明确各自策略.

### 7.1 锁画像矩阵 (9 行 × 5 列)

| 层 | 内容 | 并发模型 | 锁源 | Sync 写影响 |
|----|------|----------|------|------------|
| **L1** | JSONL (`agent-transcripts/<uuid>/*.jsonl`) | text append-only, **无 formal lock** | none | sync 通常**不**写 L1 (它是 CLI/Desktop 自己在写). 偶尔 v0.2.1 `delete_session` 走 `remove_dir_all`, 不撞中间. |
| **L2** | SQLite `store.db` (in `~/.cursor/chats/<md5>/<uuid>/`) | SQLite WAL, **整文件 single-writer** | `cursor-agent` (只在它跑的时候) | 检测到 `cursor-agent` PID → 拒绝写. 用户关掉重试. |
| **L3** | SQLite `state.vscdb` (在 `~/.config/Cursor/User/globalStorage/`) | SQLite WAL, **多 Cursor 进程 + cursor-server long-linger** | Cursor Desktop 主进程 + 多个 helper (fileWatcher / extensionHost / ptyHost / sandbox) + **cursor-server (默认 5min idle auto-shutdown)** | 被动等 idle + 主动 SIGTERM cursor-server + re-detect → 写. |

> 单进程 L3 + cursor-server 一起构成 cursor 总进程组: `pgrep -af "cursor-server|cursor-agent|Cursor --type=|/Cursor --updated|Cursor-bin"`.

### 7.2 关键差异: L2 vs L3 的 lock 释放速度

| | L2 (store.db) | L3 (state.vscdb) |
|---|---|---|
| 写锁持有者 | cursor-agent 单进程 | Cursor Desktop 主进程 + 多个 helper |
| 用户关闭行为 | `--resume` 一句话结束 | **整个 app** (可能用户只是切到 Mac 了) |
| 锁自然释放 | 几乎立刻 (`cursor-agent --resume <uuid>` 命令级) | **慢** (cursor-server 默认 5min idle 后才退) |
| 适合策略 | **硬锁**: 拒绝写 | **被动 + 主动 + bypass**: 等 / kill / staging |

### 7.3 cursor-server lifecycle 子节

> 用户上一轮对话已经验证: `cursor-server` 带 `--enable-remote-auto-shutdown` 启动, **默认 5 分钟 idle 后自动退出**.

#### 启动 flag 含义

`cursor-server --enable-remote-auto-shutdown` (Cursor v1.2+ 默认开启):
- 监听端口: SSH-over-WebSocket on `127.0.0.1:9222` 等
- **idle timeout**: 默认 `300s` (5 分钟) — 无 SSH 连接 5min 后自动退
- 关闭后下次再需要时由 `Cursor-bin --type=` 重新拉起

#### 进程树 (实测, 用户机器)

```
cursor-server (parent, 端口 9222)
  ├─ cursor-server: main              ← JS / Node 主进程
  ├─ cursor-server: worker             ← per workspace worker
  ├─ cursor-server: gpu-process
  ├─ cursor-server: fileWatcher
  ├─ cursor-server: extensionHost
  └─ cursor-server: ptyHost / sandbox (各一个)
```

`pgrep -af "cursor-server"` 会匹配这一组的所有 6-7 个进程.

#### 我们的策略 (现状 v0.2-alpha / v0.2.1 + 未来 §7.5)

```
[1] write_layer2/L3 之前
    ↓
[2] core::process::cursor_processes_running()  ← 现成, v0.2-alpha ✅
    ├─ 0 hit → 没事, 直接 [6]
    └─ N hit:
        │
        ├─ cursor-agent 命中 → refused "请关 cursor-agent 再试"
        │  (用户操作, 不主动 kill; v0.2-alpha 拍板)
        │
        ├─ Cursor --type= / Cursor-bin / /Cursor --updated 命中 → 用户活着在用
        │  → refused "请关闭 Cursor" (用户操作)
        │
        └─ cursor-server 命中 (用户已经离开)
            │
            ├─ [3.1] 走 wait_for_idle_or_signal(300s) ← 默认 5min
            │       (实测: SSH keepalive 5min 一过 cursor-server 自然退)
            │
            ├─ [3.2] 5min 后还在 → SIGTERM cursor-server parent PID
            │       (parent 退, children 跟着退)
            │
            └─ [3.3] re-detect → 还命中 → refused
                    └─ 强制进 staging bypass (§7.6)
```

### 7.4 Rust signatures (新建 `core::lock` 模块, 升级自 `core::process`)

```rust
// ── lock.rs (NEW) ────────────────────────────────────────

use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum LockKind {
    CursorAgent,      // L2 写者
    CursorDesktop,    // L3 写者群
    CursorServer,     // L3 long-linger
    Unknown,
}

#[derive(Debug, Clone)]
pub struct LockHolder {
    pub kind: LockKind,
    pub pid: u32,
    pub cmdline: String,
    pub idle_secs: Option<u64>,        // 仅 cursor-server; 用户 SSH 连接空闲时间
}

/// 升级自 core::process::cursor_processes_running. 现在返回结构化
/// (kind + pid + idle_secs), 而不是 plain `Vec<String>`.
///
/// 调用方应自己比较 cursor-server idle_secs > 300 决定是否 kill.
pub fn detect_lock_holders() -> Vec<LockHolder>;

/// 等待 cursor-server idle 超过 `timeout`, 或者 timeout 内它退出了.
/// `timeout` 推荐 300s. 返回 Ok(()) 表示 idle, Err 表示超时.
pub async fn wait_for_idle_or_signal(
    timeout: Duration,
) -> anyhow::Result<()>;

/// Send SIGTERM to a cursor-server parent PID. Returns Ok(()) when the
/// process actually exited (reaped). Refuses other kinds for safety.
pub fn sigterm_cursor_server(pid: u32) -> anyhow::Result<()>;

/// Bypass lock check entirely. Use only when the normal path fails
/// AND user has manually verified no Cursor process is using the
/// target db. Writes to a tmpdir, atomic_rename to target. WAL
/// still requires the target not to be held.
pub fn staging_write(
    db_path: &Path,
    new_db_bytes: &[u8],
) -> anyhow::Result<()>;
```

### 7.5 跟 v0.2-alpha `core::process` 的兼容关系

v0.2-alpha / v0.2.1 已经实现的 `core::process::cursor_processes_running()`:

```rust
// ── process.rs (v0.2-alpha, 117 行) ─────────────────────
const PATTERNS: &[&str] = &[
    "Cursor --type=",
    "/Cursor --updated",
    "Cursor-bin",
    "cursor-server",
    "cursor-agent",
];

pub fn cursor_processes_running() -> Vec<String> {
    // pgrep -af <pat> for each, self-filter, union vec
}
```

> **拍板时间**: 2026-07-04.
>
> **未来路线**: v0.3.x 把 `core::process` (字符串 vec) 升级成 `core::lock` (结构化 LockHolder). 旧 API 保留为 deprecated wrapper, v0.3+ 调用方迁过去. 不会破坏 v0.2.1 的兼容性.

### 7.6 staging bypass 模式 (escape hatch)

> 当正常 lock 检测 + SIGTERM 都失败, 而用户**已经手动确认** Cursor 没在用 (e.g. 远程 SSH 登不上, 但确认 Cursor 死了), 提供 `--force-staging-write` 紧急路径.

具体:
- 写一次完全 tmpdir (跟正常 write 路径一样)
- atomic_rename 到 target
- **不**调 `cursor_processes_running()`
- **不**调 `wait_for_idle_or_signal`
- 但 `backup_existing` 仍然执行 (跟正常路径一致)
- `post_write_verify` 仍然执行 (跟正常路径一致)

UI 入口: `(开发) 诊断 → 强制 staging 写入`. 不推荐普通用户用. v0.3+ 待拍板是否加 UI 入口.

### 7.7 跨调用链的保 atomicity 保证

| 操作 | atomic 保障 |
|------|-------------|
| write 单 SQLite db | `tmpdir.copy → atomic_rename` (sync.rs:1013 已实现) |
| write 跨 fs (`cp` + `rename`) | 退化为 `copy` + `rename`, 不是 atomic, 但 v0.3+ 用 `rename(2)` 单 fs 还是 atomic |
| backup 文件 | `rename` 同 fs, 同上 |
| outbox flush | 用 `mv .processed/<ts>/` 而不是删, 留 7 天 |

---

## §8 Hub-and-Spoke 与多端接力

### 8.1 Hub 不是必须的: 每个 machine 是 first-class citizen

> **拍板时间**: 2026-07-03.
>
> 旧版默认 "host machine = hub, 其它是 spoke", 前提是 host 必须在线. 这跟"出门带笔记本"场景冲突. 新版**每台机器平等**: 都有 `~/.bettercursor/`, 都跑同样代码, 都 push 也都 pull. **没有 master/slave**.

但仍然有"哪个端创建"的 weak 概念: 每份 snapshot 带 `source_endpoint.host` 标识. UI 可以选 "show only local sessions" 过滤.

### 8.2 Hub-and-Spoke 拓扑 (实际是 P2P mesh)

```
                ┌─── home desktop ───┐
                │                    │
              pull                    pull
                ↓                    ↓
        ┌─── laptop (Mac, Mac user) ┌──────┐
        │                            │      │
        └─── mobile (temporary) ────┘      │
                                            │
                              ┌─── remote dev server ───┘
                              │
                              pull
                              ↓
                            (没有 bettercursor, SSH 手动 cp)
```

- 每条 sync 关系**都是双向** (push + pull), 没有强制 hub.
- "home" 是 UX 概念 (用户视角的主机), 不是技术主从. home 离线 = 其他机器继续跑.

### 8.3 接力时序 (Mac → Linux → Mac)

```
t=0   [Mac]   用户开 session S, 写 bubble 1, 2
t=1   [Mac]   ~/.bettercursor/snapshots/mac-host/S-<t0>.json 已写
t=2   [Mac]   5min 后 daemon_loop 把 S 推到 outbox/<linux-host>/
t=3   [Linux] pull, 看到 S, 写到 snapshots/<mac-host>/
t=4   [Linux] 走 §6 classify → New → import as-is
              → 触发 v0.2-alpha sync_session: 把 S 写进 L2 + L3
t=5   [Linux] 用户在 Linux CLI 接续: cursor-agent --resume <S>
t=6   [Linux] 写 bubble 3, 4
t=7   [Linux] snapshots/<linux-host>/S-<t6>.json 已写
t=8   [Linux] daemon_loop 推到 outbox/<mac-host>/
t=9   [Mac]   pull, 看到 S 更新, 写到 snapshots/<linux-host>/
t=10  [Mac]   classify → IncomingNewer (因为 bubble 3, 4 是 Linux 后写的)
              → auto_archive_before_overwrite(S, "before_overwrite")
              → §9 写流程: L2/L3 + L3 都要更新
t=11  [Mac]   用户在 Mac Sidebar 看见 S, bubble 1-4 全在
```

### 8.4 关键不变量

> **Pull always safe**: 拉对方 snapshot 是只读对自己. `~/.bettercursor/snapshots/<other-host>/` 是 append-only 落盘, 不影响本机 unified.db.
>
> **Push needs outbox or lock-clear**: 推到对方意味着对方要写 Cursor L2/L3 或 unified.db. 当对方离线或对方 Cursor 持有 L3 锁, push 走 outbox; 等 5min idle 才能落地.

### 8.5 UI 状态机 (4 态)

```
                    按 "Sync Now"     sync 完成
              ┌─────────────────┐      ▲
              ▼                 │      │
  ┌──────┐                    ┌──────┴───────┐
  │ idle │                    │   syncing    │
  │      │                    │  (带 spinner)│
  └──────┘                    └──────────────┘
       ▲                              │
       │                              │ 检测到 Diverged
       │                              ▼
       │                       ┌──────────────┐
       │                       │  conflict    │
       │                       │  (3-way UI)  │
       │                       └──────┬───────┘
       │                              │ 用户 Apply / Cancel
       │                              ▼
       │  远端不可达              ┌──────────────┐
       └─────────────────────────│   pending    │
         (offline → outbox)       │ (outbox in)  │
                                  └──────────────┘
```

UI 状态值: `idle | syncing | conflict | pending`. React 端 `src/store/syncStore.ts` 持有它, banner 显示对应文案.

### 8.6 不做 master/slave, 不做 CRDT

> **明确拒绝**:
> - master/slave (强制 hub 在线, 跟 v0.2-alpha decision 矛盾)
> - CRDT / vector clock (over-engineering, 我们 bubble-level diff + LWW 已经够用)
> - Always-on long-lived connection (跟 SSH 短连接哲学冲突)

---

## §9 Cursor 集成细节

> v0.2-alpha / v0.2.1 已经实现了大部分 write 路径 (在 `core::sync` + `core::inject`), 本节是 reference 而非新设计.

### 9.1 写流程 (7 步, v0.2-alpha + v0.2.1 已落地)

```
[1] cursor_processes_running()    (§7) ─── hit → skipped="cursor_running"
[2] backup_existing(target_db)    (.backup_<ts> 兄弟文件)
[3] temp_copy 到 staging dir       (拷贝 main + -wal + -shm)
[4] open with WAL + busy_timeout
[5] apply mutation (core::inject::Mutation enum)
[6] atomic_rename(staging, target)  (sync.rs:1013)
[7] post_write_verify:
      PRAGMA integrity_check;
      PRAGMA wal_checkpoint(TRUNCATE);
```

### 9.2 读流程 (6 步, WAL-safe temp copy)

```
[1] 拿到 db_path (Layer 2 / Layer 3)
[2] temp_copy:
     copy db_path       → /tmp/.../main.db
     copy db_path-wal   → /tmp/.../main.db-wal
     copy db_path-shm   → /tmp/.../main.db-shm
[3] open with OpenFlags::SQLITE_OPEN_READ_ONLY
[4] PRAGMA query (canonical scan / scan_layer2_into 等)
[5] close + drop temp dir (立刻)
```

理由: 永远不直接打开 live Cursor db, 避免撞 cursor-server 的 WAL 持有.

### 9.3 sidecar 三件套 (跨 fs 关键)

| 文件 | 用途 | 必须一起复制 |
|------|------|------------|
| `state.vscdb` (main) | SQLite data | ✅ 必须跟 -wal / -shm 一起 |
| `state.vscdb-wal` | WAL log | ✅ |
| `state.vscdb-shm` | Shared memory index | ✅ |

> **铁律**: 任何涉及 Cursor db 的 temp copy / sync, 必须三件套一起. 缺一个会读到 stale snapshot (sync.rs:1013 `with_sidecar_suffix` 处理).

### 9.4 跨 fs atomic_replace

```
同 fs:   tmpdir/db → atomic_rename(target)         ← 单 syscall, atomic
跨 fs:   tmpdir/db → fs::copy + rename             ← 不是 atomic, 但 ok 因为我们本来就有 backup
```

`core::sync::atomic_replace` (sync.rs:1013) 已实现.

### 9.5 写后 verify (PRAGMA integrity + wal_checkpoint)

```rust
pub fn post_write_verify(db_path: &Path) -> anyhow::Result<()> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    let integrity_ok: String = conn.query_row(
        "PRAGMA integrity_check",
        [],
        |row| row.get(0),
    )?;
    if integrity_ok != "ok" {
        anyhow::bail!("integrity_check: {integrity_ok}");
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(())
}
```

> v0.2-alpha 已经实现了 `PRAGMA integrity_check` + `wal_checkpoint(TRUNCATE)` (在 `core::sync::write_layer2` / `write_layer3` 完成之后).

### 9.6 已废弃的离线两阶段路径 (历史, v0.2.1 已删除)

> **历史** (v0.2-alpha 设计了 scripts/apply.py + queue 目录做"离线两阶段 inject", 但 v0.2.1 完工时完全废弃):

```
旧路径 (v0.2-alpha 内测用过一段时间, 2026-07-04 删除):
  ~/.bettercursor/queue/<uuid>.json          (生成 snapshot)
  → scripts/apply.py --db state.vscdb --input queue/<uuid>.json  (人工跑)
  → atomic_rename(target) → post_write_verify

原因 (commit e4902d6): 跨设备 rename Errno 18 (EXDEV) 频繁触发; 写一次要
两步反而比 inline-write 容易出错. v0.2-alpha 内测发现 inline (Rust 进程内
走 Mutation enum) 更可靠, scripts/apply.py 504 行 + tests/test_apply_atomic_rename.py 177 行全部删除.

新路径 (现在): §9.1 inline in Rust process, 走 core::inject::Mutation enum,
不再有 script. 详见 sync.rs::sync_session / write_layer2 / write_layer3.
```

### 9.7 Rust signatures (这是 reference, 不重复 sync.rs 已写的实现)

```rust
// ── Reference signatures ────────────────────────────────
// 实际实现见 src-tauri/src/core/sync.rs + core/inject.rs

pub fn safe_temp_copy(db: &Path) -> anyhow::Result<TempDb>;
pub fn atomic_replace(staging: &Path, target: &Path) -> anyhow::Result<()>;
pub fn post_write_verify(db: &Path) -> anyhow::Result<()>;
pub fn backup_existing(path: &Path) -> anyhow::Result<std::path::PathBuf>;

/// `Mutation` 是 core::inject 暴露的 enum, sync.rs 调用:
///   Mutation::InsertBubble { ... }
///   Mutation::UpdateComposerData { ... }
///   Mutation::PatchWorkspaceIdentifier { ... }
pub fn apply_mutation_inline(conn: &Connection, m: &Mutation) -> anyhow::Result<()>;
```

### 9.8 agentKv 与 L3 写路径补全 (cursaves 对标)

> cursaves 文档与实测均表明: 无 `agentKv:blob:{hex}` 时 Desktop agent `--resume` 会报 **Blob not found**. bettercursor v0.2-alpha 的 `write_layer3` 尚未系统写入 agentKv; v4 snapshot `raw_blobs` 也依赖这块.

| 能力 | cursaves 参考 | bettercursor 落点 | 状态 |
|------|--------------|------------------|------|
| 从 composerData / bubbles 提取 agent blob id 列表 | `export.py::_extract_agent_blob_ids` | `sync.rs` protobuf walker 扩展 或新 `core/agent_blobs.rs` | ⚪ |
| 导入/补层时写入 `agentKv:blob:{hex}` | `importer.py` agent_blobs 段 | `write_layer3` / `inject.rs` | ⚪ |
| 从 snapshot 修复缺失 blob | `importer.py::repair_missing_blobs` | v4 import + Doctor command | ⚪ |
| L3 `write_batch` + backup 保留 N 份 | `db.py::write_batch` / `backup_db(keep=2)` | 扩展 `storage.rs` 写侧; 统一 `sync.rs` backup 策略 | ⚪ 部分 |
| DB fingerprint 跳过无效全量 rescan | `watch.py::_get_db_fingerprint` | `watcher.rs` debounce 前 cheap check | ⚪ |
| L3 purge (删 global DB 行) | `importer.py::purge_chats` | **不做** — PRD 刻意跳过 L3 delete, 避免损坏 Electron |

**写路径铁律不变**: 仍走 §9.1 七步 (进程锁 → backup → staging → mutation → atomic_rename → verify). agentKv 写入是 mutation 扩展, 不是旁路直写 live DB.

---

## §10 实现路线图

> ✅ **v0.2.1 + v0.2.2 + v0.2.3 + v0.2.5 + v0.2.6 + v0.2.6 housekeeping 已完工** (2026-07-04). v0.2.5 / v0.2.6 housekeeping 是**旁路 housekeeping**, 不动 sync 架构; v0.2.6 真正把 §4 transport trait 引进来 (T2 SSH/rsync 一份, 0 新 Cargo dep). 后续 milestone: **v0.3.0 大版本** (开本文件所有 §2-§7 的能力) → v0.3.1 (UI 层).

### 10.1 路线图总表

| Milestone | 任务 (本 doc §) | 估时 | 状态 |
|-----------|----------------|------|------|
| **v0.2-alpha** (2026-07-03) | v0.2-alpha: 单 session L2/L3 补层 + 锁检测 | 1d | ✅ 已落地 |
| **v0.2.1** (2026-07-04) | v0.2.1: `fix_orphans` + `delete_session` + Lock 模块独立 | 1d | **✅ 已落地 2026-07-04** |
| **v0.2.2** (2026-07-04) | v0.2.2: 对话记录展开 — L1+L2+L3 三路合并 (`canonical::merge_bubbles_three_way`) + bubble-id 对账 + 字段级 LWW + `<MessageList>` 薄包装 (sticky header + 三态文案 + 浮动跳转底部 + stable key) | 1.5d | **✅ 已落地 2026-07-04** |
| **v0.2.3** (2026-07-04) | v0.2.3: 后台 sync loop 收尾 — `refresh_sessions` → `sync_now` 改名; `watcher_status.last_scan_at_ms` 暴露给前端; `<SyncNowButton>` (立即扫描, 替代 SessionTree 内联 refresh 按钮); `<SyncStatusBadge>` ("● 自动同步 · Xs 前", 1Hz local tick + 5s poll). 不复活 `auto_sync_enabled` toggle (沿用 #103 拍板). | 1.5d | **✅ 已落地 2026-07-04** |
| **v0.2.5** (2026-07-04) | v0.2.5: 跨平台打包 + i18n (旁路 housekeeping) — version bump 三件套 (0.1.0 → 0.2.5) + `bundle.macOS` (未签名 dmg, Mac 10.15+) + `bundle.linux.deb.depends` + react-i18next (zh-CN/en, ~110 条 UI 字符串) + `<LanguageSwitcher>` (header `<select>`, localStorage 持久化) + GitHub Actions release workflow (ubuntu+macos+windows matrix, tag `v*.*.*` 触发, softprops/action-gh-release@v2). **不动 sync 架构**. | 1.5d | **✅ 已落地 2026-07-04** |
| **v0.2.6 housekeeping** (2026-07-04) | v0.2.6 旁路 housekeeping — CI matrix 加 `macos-13` (Intel x64 dmg 跟 Apple Silicon dmg 一起出) + Node 20 → 22 + vitest 2 + jsdom 25 + `@testing-library/react` 16 + 15 case 测 `<SyncStatusBadge>` / `<BrokenBadge>` i18n-aware fallback. **零业务代码改动**. | 0.5d | **✅ 已落地 2026-07-04** |
| **v0.2.6** (2026-07-04) | **跨设备 sync — Transport trait 初版 (§4)**. `Transport` trait 4 方法 (push / pull / list_remote / endpoint_id) — **同步 trait** (有意识偏离 §4.4 spec 的 `async_trait`, v0.3.0 上 outbox 时再迁). 1 个 impl: `SshRsyncTransport` (T2, 调系统 `ssh` / `rsync`, **0 新 Cargo dep**). 最小 `SessionSnapshot` 载体 (8 字段, metadata-only, 不含 bubbles/blobs). `~/.bettercursor/transports.json` peer 配置 (新文件, 跟 config.json 分开). 4 个 Tauri 命令 `transport_list_peers` / `transport_test` / `transport_push` / `transport_pull`. **无 UI** (SyncPeersDialog 推迟到 v0.3.0). 20 个 Rust 单元测试 + `tests/fixtures/fake-{ssh,rsync}.sh` mock. | 3-3.5d | **✅ 已落地 2026-07-04** |
| **v0.3.0 PR-1** (2026-07-04) | **`~/.bettercursor/unified.db` (§3) — read-cache + archive + sync_runs**. 8 表 (`schema_version` / `sessions` / `bubbles` / `bubbles_fts` / `blobs` / `composer_data` / `sync_runs` / `archive` / `conflicts`) + FTS5 虚表 (无 triggers 手动维护, `unicode61` tokenizer) + `UnifiedDb::rebuild_from_cursor_state` 幂等 ingest + `record_archive` / `record_conflict` / `record_sync_run` / `finish_sync_run` / `search_bubbles` / `delete_session_row` / `unresolved_conflicts` helpers + `paths::unified_db_path()`. `Bubble.parent_bubble_id: Option<String>` + `ComposerData { full_json, subset_json }` + `CanonicalSession.{composer_data, composer_id}` + `Sources::preferred_endpoint_kind()` + `Sources::preferred_source_path()`. **Migration A coexist**: v0.2.6 inline-write 路径保留, 4 个 hook 点 (`sync_session` / `fix_orphans` / `delete_session` / `sync_now`) 同步写 unified.db. **0 新 Cargo dep** (rusqlite + bundled + sha2 + hex 已 in). 8 单元测 + 4 canonical 字段测 = ~12 case PR-1 阶段. | 3-4d | **✅ PR-1 已落地 2026-07-04** |
| **v0.3.0 pre-PR-2** | **读路径补全 (§2.8 + §11.5)**: `extract_l3_bubble_text` + `extractToolCalls` 移植; Cursor 3.0+ session discovery (`composer.composerHeaders` / `selectedComposerIds` / `composerChatViewPane.*`); timestamp fallback (spec 010); cursor-history spec 010–013 → Rust parity fixtures. **不**改 Transport / unified.db schema. | 2-3d | **✅ 已落地 2026-07-05** |
| **v0.3.0 PR-2** | snapshot codec v4 (§2, bubbles / blob_refs / raw_blobs, `SNAPSHOT_VERSION=4`) + `Transport` trait 转 `async_trait` (§4) + `ConflictClass` 5-way enum (§6, New/Identical/IncomingNewer/LocalAhead/Diverged) + `conflict::classify` / `bubble_diff` / `auto_merge` / `auto_archive_before_overwrite` + `lib::transport_pull` 走 v4 codec + unified.db upsert + Conflict 分类 (New/Identical/IncomingNewer → upsert; LocalAhead → 跳过; Diverged → auto_merge + archive). **含 §9.8 agentKv 写入**. `core::transport::snapshot` 改名 `core::transport::snapshot_meta` (给新 `core::snapshot` 让位). 冲突算法参考 `vendored/cursaves/cursor_saves/importer.py::_check_conflict`. 新增 2 个 Cargo dep: `tokio = "1"` (full features) + `async-trait = "0.1"` (~1.5MB binary 增量, 跟 v0.3.1 outbox `tokio::time::interval` 自然衔接). | 3-4d | **✅ PR-2 已落地 2026-07-05** |
| **v0.3.0 PR-2b** | Doctor 孤儿会话 + workspace 注册对齐 + SSH workspace 路径解析 + git remote 项目标识 (§11.5 中优先级项). 默认 dry-run; **不** auto-create workspace (借鉴 cursaves 注册逻辑, 不借鉴 `find_or_create_workspace`). | 2d | ⚪ PR-2 后 |
| **v0.3.1 Phase A** | `transport_push` 改发 v4 snapshot + `transport_pull` 后 `apply_session_from_snapshot` 写 Cursor L2/L3 + 双机 SSH e2e 手册 | 2-3d | **✅ Phase A 已落地** |
| **v0.3.1 Phase B** | T2a `LanTcpTransport` + mDNS 发现 + 6 位配对 → `trusted_peers` + outbox + 5min 后台 sync loop + `<SyncPeersDialog>` + `<ConflictResolveDialog>` | 5-7d | **✅ Phase B 已落地** |
| **v0.3.2** | T3 Git adapter (路线图) — 历史可视化 | 5d | ⚪ 待拍板 |
| **v0.3.3** | T4 S3 / T5 Tailscale adapter (路线图) | 4d | ⚪ 待拍板 |

总计: v0.2.x 已全部完工; v0.3.x 大版本 ~19-23 天拍板后开干.

### 10.2 依赖图

```
v0.2-alpha ✅ ──► v0.2.1 ✅ ──► v0.2.2 ✅ ──► v0.2.3 ✅
                                              │
                                              ▼
                              ┌────────────────────────────────┐
                              │  v0.2.5 ✅ (housekeeping)       │
                              │  v0.2.6 ✅ housekeeping (Intel) │
                              │  v0.2.6 ✅ (Transport trait T2) │
                              └────────────────────────────────┘
                                              │
                              (Transport trait 已初版; v0.3.0 PR-2 ✅ 转 async + tokio)
                                              │
                                              ▼
                                          v0.3.0 ✅ (unified.db PR-1 + codec v4 + conflict 5-way + async trait PR-2)
                                              │
                                              ▼
                                          v0.3.1 (outbox + conflict UI + lock + SyncPeersDialog)
                                              │
                                ┌─────────────┴──────────────┐
                                ▼                            ▼
                          v0.3.2 (T3 Git)             v0.3.3 (T4/T5)
```

- **v0.2.6 vs v0.3.0 关键差异**: v0.2.6 Transport trait 是**同步**的 (0 新 dep, 调 `std::process::Command`); **v0.3.0 PR-2 ✅** trait 已迁 async (`tokio::process::Command` + `async-trait`). snapshot codec 从 v0.2.6 的 metadata-only (8 字段, `snapshot_meta.rs`) 升到 v0.3.0 的完整 v4 (`core/snapshot.rs`, bubbles / blob_refs / raw_blobs); pull 优先 v4 decode, fallback metadata-only.
- **v0.3.0 是分水岭**: 用户可以选 v0.2.6 (一两个月能用) 永久, 也可以跳过 0.2.x 直接上 0.3.0 (但要承担 5-6 天停更).
- **不要并行**: T3 / T4 / T5 都是 v0.3+ 才拍板; 0.2-0.3.x 阶段就只做 T2 SSH.

### 10.3 不在 v0.2.x / v0.3.x 路线图 (明确)

- ❌ 移动端原生 app (mobile 只能 T0 手动 cp)
- ❌ 加密 integrated (用户自管 FileVault / LUKS / 7zip)
- ❌ centralized SaaS dashboard
- ❌ CRDT / vector clock

---

## §11 关键文件清单

### 11.1 文件表 (8 行, SHIPPED vs NEW)

| 路径 | 状态 | 行数 (估算) | 对应 doc 节 |
|------|------|------|-------|
| `src-tauri/src/core/process.rs` | ✅ SHIPPED (v0.2-alpha + v0.2.1) | 117 | §0.5, §7.5, §7.3 |
| `src-tauri/src/core/sync.rs` | ✅ SHIPPED (v0.2-alpha + v0.2.1) | 1453 | §0.5, §9 |
| `src-tauri/src/core/inject.rs` | ✅ SHIPPED (compose_* + Mutation enum) | ~600 | §9.7 |
| `src-tauri/src/core/paths.rs` | ✅ SHIPPED (v0.1) | 138 | §9 路径 |
| `src-tauri/src/core/storage.rs` | ✅ SHIPPED (v0.1) | 200 | §9.2 WAL-safe read |
| `src-tauri/src/core/canonical.rs` | ✅ SHIPPED (v0.1, scan_all → unified.db 回填) | ~1700 | §3.6 rebuild_from_cursor_state |
| `src-tauri/src/core/snapshot.rs` | ✅ SHIPPED (v0.3.0 PR-2) | ~290 | §2 codec v4 |
| `src-tauri/src/core/unified.rs` | ✅ SHIPPED (v0.3.0 PR-1 + PR-2 upsert) | ~1000 | §3 |
| `src-tauri/src/core/transport/mod.rs` | ✅ SHIPPED (v0.2.6 + v0.3.0 async) | ~170 | §4 trait 定义 |
| `src-tauri/src/core/transport/snapshot_meta.rs` | ✅ SHIPPED (v0.2.6 metadata-only; push 仍用 8 字段) | ~250 | §2 (8-field summary) |
| `src-tauri/src/core/transport/ssh.rs` | ✅ SHIPPED (v0.2.6 + v0.3.0 tokio async) | ~410 | §5 SSH/rsync default impl |
| `src-tauri/src/core/transport/config.rs` | ✅ SHIPPED (v0.2.6 — TransportConfigFile + PeerConfig) | ~190 | §5.3 `~/.bettercursor/transports.json` |
| `src-tauri/tests/fixtures/fake-ssh.sh` | ✅ SHIPPED (v0.2.6) | ~40 | test fixture |
| `src-tauri/tests/fixtures/fake-rsync.sh` | ✅ SHIPPED (v0.2.6) | ~50 | test fixture |
| `src-tauri/src/core/transport/git.rs` | 🆕 NEW (later, v0.3.2) | — | §4 T3 |
| `src-tauri/src/core/conflict.rs` | ✅ SHIPPED (v0.3.0 PR-2) | ~215 | §6 |
| `src-tauri/src/core/lock.rs` | 🆕 NEW (升级自 `core::process`, 保留 v0.2.x 兼容) | — | §7.4 |
| `src-tauri/src/cli/bettercursor.rs` | 🆕 NEW binary | — | §5.6 (CLI push / apply / outbox) |

### 11.2 前端 (src/)

| 路径 | 状态 | 备注 |
|------|------|------|
| `src/components/SessionDetail.tsx` | ✅ v0.2.1 | sync banner + 修复 + 删除 (内联) |
| `src/components/SessionTree.tsx` | ✅ v0.2.1 | WrenchButton 批量 fix_orphans |
| `src/lib/tauri.ts` | ✅ v0.2.1 | `syncSessionLayer23` + `fixOrphans` + `deleteSession` wrappers |
| `src/store/sessionStore.ts` | ✅ v0.1 | Zustand session 列表状态 |
| `src/components/SyncBanner.tsx` | 🆕 v0.3.1 待拆 | 当前内联在 SessionDetail.tsx |
| `src/components/ConflictResolveDialog.tsx` | ✅ v0.3.1 | §6.7 冲突列表 + 接受合并/跳过 |
| `src/components/SyncPeersDialog.tsx` | ✅ v0.3.1 | mDNS 发现 + 配对 + push/pull |
| `src/store/syncStore.ts` | ✅ v0.3.1 | 配对/发现/冲突状态 |

### 11.3 已删除 (历史)

> ⚠️ 不要在新代码 / 新 doc 里引用这些:

| ~~路径~~ | 用途 | 删除原因 |
|---------|------|---------|
| ~~`scripts/apply.py`~~ (504 行) | v0.2-alpha 离线两阶段 inject | v0.2.1 inline 路径不再需要 (commit `e4902d6`) |
| ~~`tests/test_apply_atomic_rename.py`~~ (177 行) | apply.py 配套测试 | 随 apply.py 一起删 |

### 11.4 Python reference (保留, 不进 runtime)

| 路径 | 行数 | 对应新 doc 节 |
|------|------|--------------|
| `bettercursor/conflict.py` | 96 | §6.2 (5-way 分类) |
| `bettercursor/snapshot.py` | 198 | §2 (旧 v3 参考, codec 已换) |
| `bettercursor/blob_dag.py` | 188 | (无需在新 doc 引用, v0.2-alpha 已 inline 到 sync.rs) |
| `bettercursor/layer2.py` | 183 | (同上) |
| `bettercursor/layer3.py` | 189 | (同上) |

> Python 仅作**设计参考**, 不要尝试双跑 (新 doc 不强行兼容 Python codec).

### 11.5 vendored 上游借鉴索引 (2026-07-04)

> 子 agent 对 `vendored/cursaves/` + `vendored/cursor-history/` 与主项目代码级对比的结论. **算法级重写**, 不搬运源码 (cursaves AGPL-3.0).

#### 三者定位

| 项目 | 栈 | 存储范围 | 强项 |
|------|-----|---------|------|
| cursaves | Python CLI | L3 + workspace 索引 | 导入导出、冲突、agentKv、Doctor、git 同步 |
| cursor-history | TS 库 + CLI | L3 (workspace + global) | bubble 文本提取、session recovery、spec+测试 |
| bettercursor | Tauri + Rust | **L1 + L2 + L3** | 三路合并、L2↔L3 补层、transport、GUI、进程锁 |

#### 高优先级 — 建议落地顺序

| # | 借鉴什么 | 上游文件 | bettercursor 落点 |
|---|---------|---------|------------------|
| 1 | L3 bubble 完整文本 + toolCalls | `cursor-history/src/core/storage.ts` | `canonical.rs` — 见 §2.8 |
| 2 | Cursor 3.0+ 多源 composer ID 发现 | cursaves `paths.py::get_workspace_composer_ids`; cursor-history `storage.ts::listSessions` | `canonical.rs::scan_layer3_into`; 可选 `core/workspace.rs` |
| 3 | Timestamp fallback | cursor-history spec 010 | `decode_l3_bubble_blob` + merge 排序 |
| 4 | Parity fixtures | cursor-history `tests/unit/storage.test.ts` + spec 012/013 | `canonical.rs::tests` + `tests/fixtures/cursor-history/` |
| 5 | agentKv 提取/写入/修复 | cursaves `export.py` / `importer.py` | `sync.rs` / §9.8 |
| 6 | L3 write batch + backup | cursaves `db.py` | `storage.rs` 写侧 |
| 7 | 冲突五态 + diverged fork | cursaves `importer.py::_check_conflict`; `bettercursor/conflict.py` | `core/conflict.rs` — §6 |
| 8 | v4 ancillary 字段 (messageContexts, checkpoints, agentBlobs) | cursaves `export.py::export_conversation` | `core/snapshot.rs` — §2 |

#### 中优先级

| 借鉴什么 | 上游 | 落点 |
|---------|------|------|
| Doctor 孤儿审计/恢复 | cursaves `doctor_audit` / `doctor_recover` | 新 Tauri command + `core/doctor.rs` |
| Git remote 项目标识 | cursaves `paths.py::get_project_identifier` | `paths.rs` + snapshot `project_slug` |
| SSH workspace 路径 | cursaves `paths.py::list_all_workspaces` | `paths.rs` |
| 导入后 workspace 注册 (不自动建 workspace) | cursaves `importer.py::_register_in_*` | 对齐 `inject.rs` |
| DB fingerprint 减扫 | cursaves `watch.py::_get_db_fingerprint` | `watcher.rs` |
| 跨设备路径重写 | cursaves `importer.py::rewrite_paths` | v4 import / `sync.rs` |
| TokenUsage / 降级标记 | cursor-history spec 009/012 | `Bubble` / `Conversation` / UI |
| 轻量 spec 驱动 (parser 契约) | cursor-history `specs/` 结构 | `docs/PARSER_SPEC.md` (仅协议级变更时) |

#### 不建议借鉴

| 项 | 原因 |
|----|------|
| 直接 import cursaves Python | AGPL-3.0 |
| cursaves v3 gzip 作主 snapshot 格式 | 已选 v4 bubble-level plain text (§2.4) |
| `find_or_create_workspace()` | bettercursor 要求用户先在 Cursor 打开项目 |
| L3 purge / 强制删 global DB | PRD 跳过 L3 delete |
| cursaves 60s mtime 轮询 watch | 已有 notify + debounce |
| cursor-history 整体 TS 库 + sidecar | 已选 Tauri+Rust 单栈 |
| 仅 workspaceStorage 模型 | 丢失 L1/L2 CLI session |
| `generations` 时间窗拼 session | 质量低于 global bubble + L1 |
| live npm/pip 依赖 vendored | 只读参考 |
| Display 截断下沉存储层 | spec 013 反模式; 损害 FTS/codec |

#### bettercursor 已更强 (无需回退对齐)

- 三层存储统一 + `Sources` 三色标签
- `merge_bubbles_three_way` 字段级 LWW
- L2 orphan 修复 (`fix_latest_root`)
- 可写 sync + 进程锁 (含 `cursor-agent`) + 写前备份
- SSH/rsync transport + `unified.db`
- notify 实时监听 + Windows 支持 + 桌面 GUI + i18n
- Rust 单测覆盖 (`canonical.rs` 等)

---

## §12 决策记录

### 12.1 已拍板表 (10 条)

| # | 决策 | 理由 | 替代 | 拍板时间 |
|---|------|------|------|----------|
| 1 | snapshot = **纯文本 JSON** (不 gzip) | 3-way merge + 文本 diff + rsync --partial | cursaves v3 JSON.gz | 2026-07-03 |
| 2 | **每机本地** `~/.bettercursor/unified.db` | 离线下仍然可用 + 用户自备份 | 单一 hub DB (强制 hub 在线) | 2026-07-03 |
| 3 | transport = **SSH/rsync default** (T2) | 零第三方, 用户已有 SSH key | Tailscale mesh (强制装 daemon) | 2026-07-03 |
| 4 | 冲突解决 = **LWW + bubble-level diff** + 3-way UI | 单 LWW 会丢改动, 我们 bubble-level 更保真 | 纯 LWW (丢对话内容) | 2026-07-03 |
| 5 | 锁策略 = **硬锁 + 主动 SIGTERM** (cursor-server) | 不卡 UX, 不损坏数据 | 等用户主动关 (UI 卡住) | 2026-07-03 |
| 6 | 备份 = 用户自管 (`cp -r ~/.bettercursor/`) | plain text + SQLite, 零依赖 | 集成云备份 | 2026-07-03 |
| 7 | **v0.2.1 L3 delete 跳过** (Cursor 自己管) | workspaceStorage 不要乱写 | 强制写 L3 (损坏风险) | 2026-07-04 |
| 8 | v0.2.1 **delete = `remove_dir_all`** (无 trash) | 用户明示"直接 rm" | trash sidecar + 还原 UI | 2026-07-04 |
| 9 | v0.2.1 **`fix_orphans` 全量扫** + `.backup_<ts>` 自动留 | 防御性, 一个后端多个 UI 入口复用 | 单条入口 + 手动备份 | 2026-7-04 |
| 10 | **锁检测独立** `core::process` (后续升 `core::lock`) | 模块化, 单测覆盖 | 留在 sync.rs 内联 (难测试) | 2026-7-04 |

### 12.2 已拍板: 不做 (negative decisions)

| 不做 | 理由 |
|------|------|
| Master/slave 拓扑 | 强制 hub 在线, 跟"出门带笔记本"场景冲突 |
| CRDT / vector clock | over-engineering, bubble-level LWW 够用 |
| Always-on long-lived connection | 跟 SSH 短连接哲学冲突 |
| 加密 integrated | 交给 FileVault / LUKS / 7zip |
| Cloud dashboard / SaaS | 用户自管数据 |
| Mobile native app | 复杂度爆炸, T0 手动够 |
| 双写 Python + Rust codec | 维护成本, 不兼容 v3 gzip |

### 12.3 待拍板表 (4 条)

| 决策 | 候选项 | 评估维度 | 推荐 |
|------|--------|---------|------|
| T3 Git adapter 何时做? | v0.3.2 / 永远不做 / 留给 community plugin | 用户需求 + 维护成本 | v0.3.2 (当 ≥2 用户明确要求) |
| T4 S3 / T5 Tailscale adapter? | v0.3.3 / 永远不做 | 用户群体 + 安全审查 | 永远不做 (除非出现 ≥5 用户社区诉求) |
| v0.3.0 unified.db 是否替换 v0.2-inline-write 路径? | 替换 (大写) / 共存 (兼容) / 取舍 | 数据迁移风险 + 用户教育 | 替换 + 提供一次性 migration tool |
| v0.3.1 conflict UI 默认动作? | 单一 LWW (自动) / 严格 3-way UI (每次问) | UX 摩擦 vs 数据保真度 | 单一 LWW 默认 + "detected conflict" 通知, 点击才进 UI |

### 12.4 拍板后回写路径

> 拍板 = 在 PR description / commit message / 本表添加一行. 拍板才开干, 不在 Discord 里拍板.

拍板完成后:
1. 更新 [PRD.md §0.5](PRD.md) 状态行
2. 更新 [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) Phase 章节
3. 在本表加一行, 标注日期 + commit 哈希

---

## §13 风险与缓解

### 13.1 风险表 (5 类)

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| **Cursor 升级改 store.db schema** | 中 | 高 | 写时 backup, 失败回滚; schema 不匹配时整 session 标 error; rebuild_from_cursor_state 重新发现 |
| **L3 cursor-server linger 不释放** (用户离开超 5min) | 中 | 中 | 走 §7.3 SIGTERM 主动 kill; staging bypass escape hatch |
| **DIVERGED 数据丢失** (3-way merge apply 失败) | 低 | 高 | §6.5 强制 auto-archive before overwrite; 即使 merge 失败用户也能从 archive JSON 还原 |
| **隐私 / snapshot 泄露** (snapshot 含明文对话) | 低 | 中 | T2 SSH 走用户私网; 不强制 T5 Tailscale; 不做 T4 S3 (除非用户自建) |
| **Git adapter 仓库膨胀** (snapshot 历史) | 中 | 低 | T3 是 optional; 默认走 T2 不走 git |

### 13.2 退出策略 (从原 §11 合并)

> **如果 sync 做不下去, 我们停在以下位置**:

| 场景 | 回退 |
|------|------|
| unified.db 太重 / FTS5 太慢 | 退回 v0.2.1: 手动 sync + orphan + delete 已够日常 |
| SSH/rsync 默认失败 (用户全在 sandbox) | 退到 T0 手动文件: `scp snapshot` 用户自己想办法 |
| Tailscale 还是太多用户问 | v0.3.3 加回 T5 (Tailscale 仅作为可选) |
| 用户改主意不要 sync 任何东西 | 回到 v0.1 (只读), sync.rs + process.rs 全部删除. PRD §0.6 是事实真相 |
| v0.3.x 永远做不完 | 锁定 v0.2.4 (跨设备用了 v0.2.1 inline-write) |

### 13.3 不在 §13 的隐性风险 (灰色地带)

- **WAL 在 macOS 上的 check-summing bug** (SQLite 历史 bug): 不在本设计稿范围, Cursor 自己处理.
- **`com.docker.backend` / M-series sandbox**: 跟 §7.3 锁检测无直接关系, 但用户在 Docker 容器里跑 Cursor 时, lock 检测可能误报. 等用户具体案例再拍.
- **GPU 多端同步冲突 (Mac vs Linux 同时打开同一 session)**: 当前用 bubble-level LWW 处理, 还没发现具体数据丢失案例. 留 TODO.

---

## 附录

### A. 已落地测试覆盖 (2026-07-04 现状)

| Module | 测试数 | 备注 |
|--------|--------|------|
| `core::sync` | 49 个 + 2 `#[ignore]` live smoke (`cfa4177f`, `62eb1b04`) | 大部分是 unit, 集成靠 live smoke |
| `core::process` | 3 个 (`cursor_processes_running_returns_vec`, `pgrep_for_filters_empty_pattern`, `pgrep_for_self_reference_strippable`) | smoke + self-filter |
| `core::canonical` | 单元 + 集成 | 17 session 实测结果与 Python 一致 |
| `core::paths` | 单元 | 4 层路径 + MD5 chat_root parity test |
| `core::storage` | 单元 | `read_missing_db_errors` 等 |

### B. 关键 cross-reference (链接到其他 doc)

| 主题 | 链接 |
|------|------|
| 产品现状 | [PRD.md](PRD.md) §0 |
| 背景调研 | [BACKGROUND.md](BACKGROUND.md) §6.5 (SSH 反向推送史) |
| 实施计划 | [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) §3 (commands) |
| Tailscale 历史 | [mihomo-tailscale-fakeip-conflict.md](mihomo-tailscale-fakeip-conflict.md) (旧 v0.2 设计参考, 已不作为 default) |
| Cursor 存储路径 | [vendored/cursaves/cursor_saves/paths.py](../vendored/cursaves/cursor_saves/paths.py) (Python 参考) |
| vendored 借鉴索引 | [§11.5](SYNC_DESIGN.md) (cursaves + cursor-history 对比结论) |
| L3 bubble 解析缺口 | [§2.8](SYNC_DESIGN.md) (cursor-history `extractBubbleText` 对标) |
| agentKv / L3 写补全 | [§9.8](SYNC_DESIGN.md) (cursaves export/importer 对标) |
| cursor-history bubble 解析 | [vendored/cursor-history/src/core/storage.ts](../vendored/cursor-history/src/core/storage.ts) |
| cursor-history 测试/spec | [vendored/cursor-history/specs/010–013](../vendored/cursor-history/specs/) |

### C. 后续待补的 doc (TODO)

- [ ] 用户手册: 怎么 `cp -r ~/.bettercursor/` 备份 + 还原
- [ ] SSH key 配置示例 (`~/.ssh/config` 让 `macbook` 主机名可用)
- [ ] outbox 调试: `bettercursor outbox list` 输出说明
- [ ] Tailscale + T2 共存场景: 何时降级到 T2 (Tailscale 断网时)
- [ ] 跨 fs rename 异常诊断 (commit `e4902d6` 的 bug 复现步骤)
