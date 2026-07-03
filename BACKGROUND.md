# bettercursor — 让 Cursor 拥有 Codex 般的 session 体验

## TL;DR

把 Cursor 散落在 **多端多个存储层** 的 session 收拢，通过 Linux 宿主机上的
daemon + Mac 上反向推送的小客户端，让任意一端（Mac UI · Linux cursor-agent
CLI · Linux Desktop Electron）都能看到所有 session。

**Codex 对照**:
- Codex desktop + CLI 在**同一台机器**自动共享 session (本地存储, 不走云).
- Codex 在**不同机器**之间**不共享** — ssh 到别的机器, session 永远只在宿主机本地.
- Cursor desktop + CLI 即使在同一台机器上也不共享 — 这是 bettercursor 要修的核心问题.
- 跨设备 (Mac + Linux) 部分是通过 Tailscale mesh + 本地 daemon 补的, 跟 Codex 无关.

---

## 1. 问题陈述 (Problem)

### 1.1 你感受到的症状

- **Mac UI 写的 session 在 Linux CLI / Desktop 看不见**。
- **Linux CLI / Desktop 写的 session 在 Mac UI 看不见**。
- 切换 SSH remote workspace 配置还会让 Mac 自己产生 **空草稿残留**。
- Sidebar 时有时无，看不到全貌。

### 1.2 你的关键约束

> 任何一端都要能看见所有 session，但同一时间只会在一个端和 agent 聊天和工作。

约束消除了并发写一致性担忧。剩下唯一要解决的问题是**多端可见性同步**。

### 1.3 你的实际架构 (经过 CPU 实测确认)

**模型 A: Mac 本地 Electron + SSH 只访问文件**

```
[你 Mac 上的 Cursor Electron] ─── SSH ───→ [Linux 上的 enenzuo 项目]
       │                                          │
       │ 本地 userData                            │
       ↓                                          ↓
┌─────────────────────────────────┐    ┌────────────────────────┐
│ Mac userData (Mac 主真相)        │    │ Linux 端 (辅助真相)    │
│ ~/Library/Application Support/  │    │                        │
│   Cursor/                       │    │  ┌──────────────────┐  │
│   ├── User/globalStorage/       │    │  │ JSONL (workspace │  │
│   │   state.vscdb (10.4 MB)     │    │  │  同步副产物)     │  │
│   └── User/workspaceStorage/    │    │  │ ~/.cursor/       │  │
│       └── 16 个项目 hash        │    │  │   projects/.../  │  │
│                                 │    │  │   agent-         │  │
│ agent: 158% CPU 跑在这        │    │  │   transcripts/   │  │
└─────────────────────────────────┘    │  └──────────────────┘  │
                                       │  ┌──────────────────┐  │
                                       │  │ Linux cursor-    │  │
                                       │  │ agent CLI store  │  │
                                       │  │ ~/.cursor/chats/ │  │
                                       │  │   <md5(cwd)>/.../│  │
                                       │  │   store.db       │  │
                                       │  └──────────────────┘  │
                                       │  ┌──────────────────┐  │
                                       │  │ Linux Desktop    │  │
                                       │  │ ~/.config/Cursor/│  │
                                       │  │   User/globalStorage/│
                                       │  │   state.vscdb    │  │
                                       │  └──────────────────┘  │
                                       └────────────────────────┘
```

**关键确认** (来自 Mac 上 ps + CPU 监控):

- Mac 上跑的是本地 Electron 进程（无 cursor-server）
- Mac 上发消息时，Renderer PID 158% CPU 飙升
- Linux 上没有 cursor-server 进程在处理 agent
- Linux cursor-server 进程是给 workspace service 用的（管理文件、终端）

**CPU 监控详细时间线** (你在 Mac 上发 "帮我重构 auth.py" 时)：

| 时间 | Total CPU% | 峰值进程 | 阶段 |
|------|-----------|----------|------|
| 00:21:35 | 3.9% | — | 静默基线 |
| 00:21:43 | 5.3% | — | 你按下回车 |
| 00:21:45 | 38.4% | Renderer 21.5% | agent 开始思考 |
| 00:21:51 | 🔥 188.3% | Renderer **158.1%** | token 流式生成峰值 |
| 00:22:02 | 5.8% | — | 第一波结束（agent 回复完成） |
| 00:22:08 | 29.6% | Renderer 52.1% | 第二波（tool 调用） |
| 00:22:18 | 5.4% | — | 回到静默 |

所有飙升都是 Mac 上的 `Cursor Helper (Renderer)`，完全没有 SSH remote 侧负载。
→ **决定性证据**: agent 跑在 Mac 上。

**Mac 上的进程族**:
- `/Applications/Cursor.app/Contents/MacOS/Cursor` (主进程)
- `Cursor Helper (Renderer)` (渲染, 多实例)
- `Cursor Helper (GPU)` (GPU 加速)
- `Cursor Helper (Plugin)` (插件沙箱)
- `Cursor Helper (Network)` (网络服务)
- `Squirrel` (自动更新)
- **没有 cursor-server 进程** ← 关键

---

## 2. 完整存储架构 — **4 层独立索引**

> 之前以为只有 3 层，其实有 4 层，且各端有**完全独立**的索引。

```
LAYER 1: ~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl
         ├── 写: Mac UI (通过 SSH 写到 Linux) + Linux cursor-agent CLI + Linux Desktop
         ├── 读: 全员 (但无索引，只能按路径直读)
         └── 性质: workspace 同步副产物, 跟着项目走

LAYER 2: ~/.cursor/chats/<md5(cwd)>/<uuid>/
         ├── meta.json           {schemaVersion, hasConversation, title, ...}
         ├── prompt_history.json ["/resume", ...]
         └── store.db (SQLite)   blobs(id, data) + meta(key, value)
         ├── 写: Linux cursor-agent CLI (only)
         └── 读: Linux cursor-agent CLI --resume 列表

LAYER 3: ~/.config/Cursor/User/globalStorage/state.vscdb
         ItemTable['composer.composerHeaders']    ← Sidebar 中央索引
         cursorDiskKV['composerData:<uuid>']      ← 完整 composer 快照 (~50字段)
         cursorDiskKV['bubbleId:<uuid>:<bid>']    ← 每条消息 blob
         ├── 写: Linux Electron Desktop (only on Linux)
         └── 读: Linux Electron Desktop Sidebar

LAYER 3' (Mac): ~/Library/Application Support/Cursor/User/globalStorage/state.vscdb
         同结构, 但只属于 Mac
         ├── 写: Mac Electron Desktop (本地 SQLite)
         └── 读: Mac Electron Desktop Sidebar
```

### 2.1 空草稿过滤逻辑 (Empty Draft Filter)

每个端都把"空草稿"过滤掉，不显示在 session 列表里：

| 端 | 过滤条件 |
|---|---------|
| Layer 2 (CLI) | `exists(store.db)` AND `meta.json.hasConversation == true` |
| Layer 3 (Desktop) | `EXISTS(composerData:<uuid>)` AND `len(fullConversationHeadersOnly) > 0` |
| Layer 3' (Mac) | 同 Layer 3 |

**空草稿的产生时机**:
- CLI: 你打开 prompt 即创建 meta.json (即使没打字就退出)
- Mac UI / Desktop: 你点 "New Chat" 即创建空 composer entry (即使没输入就关闭)

Linux CLI 6 条空草稿的 prompt_history.json 内容都是 `["/resume"]` 或只有 slash command —
你打开 CLI 查 resume 然后退出，从未发过真实消息。Mac 的 4 条空草稿同理，是 UI 残留。

### 2.2 JSONL 真正来源 (修正之前的误判)

最初我以为 `~/.cursor/projects/<slug>/agent-transcripts/<uuid>.jsonl` 是
cursor-server 写的。**实际上**：

> Mac UI 通过 SSH 把 transcript 写到 Linux 项目目录（workspace 同步机制）。
> transcripts 跟着 workspace 走，不跟着 agent runtime 走。

Linux 上的 cursor-server 进程只管 workspace service（文件 IO、终端、扩展），
**不**处理 agent。Mac 上的 agent 跑完一段对话后，把 transcript 写到 Linux 上对应项目的
`.cursor/projects/.../agent-transcripts/` 路径，作为 workspace 的一部分。

所以：
- **Mac 新 session → JSONL 自动落到 Linux** (Cursor 自带, 不需要我们做)
- **Linux 新 session (CLI/Desktop) → JSONL 也在 Linux** (本地写)
- **跨端共享的就是 JSONL，但 JSONL 没有 Sidebar 索引**

### 2.3 Mac 的两个 workspaceStorage 哈希

Mac 的 `~/Library/Application Support/Cursor/User/workspaceStorage/` 里有 16 个
项目 hash，enenzuo 对应**两个**（因 SSH 配置换过）：

| hash | 角色 | enenzuo 内容 | session 数 |
|------|------|-------------|------------|
| `b0579a9bddde99b170f20d58a0f5040f` | **旧 SSH remote config** | 4 条空草稿 | 4 (全部 hasData:false) |
| `946eda0d4e927e1d340b92790f030093` | **新 SSH remote config** | 3 条有内容 | 3 (WeChat profile, Model used, Device OS) |

切 SSH 配置 = 换 workspace hash。旧 hash 不会被自动清，里面残留的空草稿会一直存在。
**这是 Mac 那 4 条空草稿的来源** — 不是真 session，是 Cursor UI 切 workspace 的产物。

---

## 3. 目标架构 — 反向推送 (Reverse Push)

**关键约束** (来自你):
- Mac 不开 SSH server
- Linux 宿主机永远在线 → daemon 跑 Linux
- Mac 通过 SSH client → 连 Linux (现有能力, Mac 一直在 SSH 到 Linux)

```
┌─────────────────────────────────────────────────────────────────────┐
│                  Linux 宿主机 (daemon 永远在)                        │
│                                                                     │
│   bettercursor-syncd (systemd --user)                               │
│   ├── 监听 ~/.cursor/projects/*/agent-transcripts/  (JSONL)         │
│   ├── 监听 ~/.cursor/chats/<md5>/.../              (Layer 2)        │
│   ├── 监听 ~/.config/Cursor/User/globalStorage/    (Layer 3)        │
│   ├── 接收 SSH 进来的 sync 请求 (Mac client 推送)                  │
│   │                                                                  │
│   └── 合并逻辑: UUID dedup + lastUpdatedAt 取最新                   │
│       ↓                                                              │
│   输出: ~/.bettercursor/sessions.json (canonical state)             │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
                                  ↕ SSH (反向推送)
┌─────────────────────────────────────────────────────────────────────┐
│                  Mac (用户日常, 偶尔睡眠)                            │
│                                                                     │
│   bettercursor-sync (launchd plist, 每 5 分钟触发)                  │
│   ├── ssh linux 'bettercursor-syncd export --project=<slug>'        │
│   ├── 接收 JSON 流 → 写 Mac state.vscdb                            │
│   └── 同时: ssh linux 'bettercursor-syncd ingest' < mac_state.dump  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

**为什么反向推送而非正向拉取**:
- Mac 不开 SSH server, Linux 不能主动连 Mac
- Mac 主动连 Linux (现有能力), Linux 被动接收
- Mac 睡眠时不影响 daemon, daemon 继续维护 Linux 端统一状态

---

## 4. 4 个端与 4 层存储的可见性矩阵

| 端 | 主要读 | 主要写 | 当前看到 enenzuo session |
|---|--------|--------|--------------------------|
| Mac Electron UI | Mac state.vscdb (Layer 3') | Mac state.vscdb + JSONL (via SSH) | 7 条 (4 空 + 3 实) |
| Linux cursor-agent CLI | Linux chats/store.db (Layer 2) | Layer 2 + JSONL | 2 条 (289c14c4, 62eb1b04) |
| Linux Electron Desktop | Linux state.vscdb (Layer 3) | Layer 3 + JSONL | 1 条 (c1ea7999) |
| Linux cursor-server (workspace only) | 不读 session | 不写 session | 0 条 |

跨端**完全不可见**。

### 4.1 enenzuo 17 条 session 的精确分布

```
总唯一 UUID: 17 条, 分布在 4 个存储层之间互相不可见
```

| UUID 前缀 | JSONL | Layer 2 (CLI) | Layer 3 (Linux Desktop) | Layer 3' (Mac) | 实际 owner |
|-----------|:-----:|:-------------:|:-----------------------:|:--------------:|------------|
| 289c14c4  | ✓ | ✓ | ✗ | ✗ | Linux CLI |
| 62eb1b04  | ✓ | ✓ | ✗ | ✗ | Linux CLI |
| 67f8008e  | ✓ | dir only | ✗ | ✓ (946eda0d) | Mac (via SSH) |
| 9bf5c838  | ✓ | ✗ | ✗ | ✓ (946eda0d) | Mac (via SSH) |
| cec4f76b  | ✓ | ✗ | ✗ | ✓ (946eda0d) | Mac (via SSH) |
| c1ea7999  | ✓ | ✗ | ✓ | ✗ | Linux Desktop |
| d7be5721  | ✓ | ✗ | ✓ | ✗ | Linux Desktop |
| 697dffa5  | ✗ | ✗ | ✓ | ✗ | 孤儿 (来源不明) |
| 2e47ed39  | ✗ | dir only | ✗ | ✗ | Linux CLI 空草稿 |
| 7fe5c8b2  | ✗ | dir only | ✗ | ✗ | Linux CLI 空草稿 |
| 8c02b36b  | ✗ | dir only | ✗ | ✗ | Linux CLI 空草稿 |
| 8ee024da  | ✗ | dir only | ✗ | ✗ | Linux CLI 空草稿 |
| c06a98c2  | ✗ | dir only | ✗ | ✗ | Linux CLI 空草稿 |
| 97db6eee  | ✗ | ✗ | ✗ | ✓ (b0579a9b 空) | Mac 旧 workspace 残留 |
| 2ec840b3  | ✗ | ✗ | ✗ | ✓ (b0579a9b 空) | Mac 旧 workspace 残留 |
| 1456604a  | ✗ | ✗ | ✗ | ✓ (b0579a9b 空) | Mac 旧 workspace 残留 |
| 4ad861df  | ✗ | ✗ | ✗ | ✓ (b0579a9b 空) | Mac 旧 workspace 残留 |

**核心观察**:
- 7 条有真实对话内容 (跨 4 个端)
- 4 条 Mac UI 空草稿 (b0579a9b workspace 残留)
- 6 条 Linux CLI 空草稿 (cursor-agent prompt 占位)
- 0 条真 session 被所有端都看到
- daemon 的目标: 让 7 条真 session 在所有 4 个端都可见

### 4.2 chat_root 是 MD5(cwd)

`~/.cursor/chats/c19d07070edc77b1fdcdaf0dfecaf97f/`

验证: `MD5("/home/eric/workspace/enenzuo")` = `c19d07070edc77b1fdcdaf0dfecaf97f` ✓

含义: 同一项目所有 CLI session 共享一个 chat_root, 用 cwd 哈希识别。
daemon 可以利用这个: 从 JSONL 路径 (`~/.cursor/projects/<slug>/`) 反推项目 cwd,
进而定位 chat_root。

---

## 5. 实施路线

### Phase 1 — Linux daemon 本地单向 (~半天)

- 已有骨架: `/home/eric/workspace/enenzuo/cursync/cursync-import.py` (待迁过来修 bug)
- 改名 `bettercursor-bridge.py`
- 监听 Linux 3 层 (JSONL + Layer 2 + Layer 3)
- 合并 → 写 Linux 的 Layer 2 + Layer 3
- **dry-run 默认**, 需 --apply 才落盘
- **验收**: dry-run 报告后你审, 通过后 apply
- **P0 子任务 (已完成)**: `adapter/fix_orphan_sessions.py` 修复 `meta[0].latestRootBlobId = ""` 的孤立会话. 扫描所有 `~/.cursor/chats/<md5>/<uuid>/store.db`, 自动找 root 候选 (不被其他 blob 引用但能传递引用最多 blob 的那个), 写入 meta. **已验证**: c1ea7999 修复后 `--resume` 工作, agent 记住全部上下文.

### Phase 2 — Linux daemon 暴露 SSH 命令 (~半天)

- `bettercursor-syncd export` → stdout JSON 流
- `bettercursor-syncd ingest < mac_dump` → 合并
- 单测: SSH 进来 export 看输出

### Phase 3 — Mac client (~半天)

- `bettercursor-sync` 脚本 (Bash + sqlite3, 不需要 Python on Mac)
- launchd plist 每 5 分钟触发
- SSH 连 Linux daemon
- 解析 JSON → 写 Mac state.vscdb
- **验收**: 重启 Mac Cursor, Sidebar 看到 Linux 那些 session

### Phase 4 — 反向推送 (~半天)

- Mac client 同时把 Mac 的 composerHeaders dump 推到 Linux
- Linux daemon ingest 时合并
- **验收**: Mac 上开新 session, 等 5 分钟, Linux Desktop Sidebar 看到

### Phase 5 (可选) — `cs` 包装

- `cs ls` / `cs new` / `cs resume` / `cs show` / `cs doctor`

### Phase 6 (可选) — 清理空草稿

- Mac 的 4 条 b0579a9b 空草稿: 不影响 Sidebar (b0579a9b 已不被激活), 但占 Mac state.vscdb 空间
- Linux 的 6 条 cursor-agent CLI 空草稿: 不影响 resume 列表, 但占 chats/ 空间
- 清理脚本 `bettercursor-clean`: 按规则扫描 + 删除
- **先不做**, 等 daemon 跑稳了再清理

---

## 6. 待回答 / 不确定

### 6.1 Cursor Cloud Sync 是否启用？

Mac 上有 7 条本地 session, Linux 上没有 — 如果 Cloud Sync 开了, 理论上 Mac 应该能从云上拉到
Linux 的 session (反之亦然)。但实测看不到, 可能:

- (a) Cloud Sync 没开 (默认 Pro 用户开启, 但可能 session 类型不被同步)
- (b) Cloud Sync 开了但只同步本地 SQLite, 不同步 JSONL/chats/
- (c) Cloud Sync 开了但有过滤规则

**如果 Cloud Sync 启用且全同步**, 我们的 daemon 大部分工作就被 Cursor 自带同步做了, 只需要补缺。
**如果 Cloud Sync 没启用或不全**, 我们的 daemon 是必须的。

### 6.2 Mac 创建 session 后, Linux 何时能看到 JSONL？

理论上 Mac 通过 SSH 写文件, 应该实时。但实测发现:
- c1ea7999 的 JSONL 在 Linux 上, SQLite 也在 Linux Desktop → 说明 Linux Desktop 自己写过
- d7be5721 同上
- 67f8008e / 9bf5c838 / cec4f76b 的 JSONL 在 Linux, 但 Mac SQLite 有 entry → **是 Mac 通过 SSH 写的 JSONL**

但 Mac 写完后, Linux 端是否立即看到? 取决于:
- Cursor 是否同步写 (vs 异步批写)
- SSH 通道延迟
- Linux 文件系统缓存

daemon 需要 inotify + poll 双重监听才能确保不漏。

### 6.3 Mac userData 走系统 SSH 凭据吗？

Mac 通过 `~/.ssh/config` 里的配置连到 Linux, 用 SSH 密钥认证。daemon 不需要新凭据,
只需知道 Mac → Linux 的 SSH 别名 (比如 `linux` 或具体 hostname)。

### 6.4 workspaceIdentifier 推断规则 (已选定 A 方案)

如果某 session 在 Mac 看到但 Linux 不知道 workspaceIdentifier (新项目), 用以下规则推断:

```python
def infer_workspace_id(mac_session, jsonl_path):
    # 1. 从 JSONL 路径 ~/.cursor/projects/<slug>/... 反推 fsPath
    slug = jsonl_path.parent.parent.parent.name  # e.g. "home-eric-workspace-enenzuo"
    if slug.startswith("home-eric-workspace-"):
        fs_path = "/home/eric/workspace/" + slug.replace("home-eric-workspace-", "")
    elif slug.startswith("home-eric-"):
        fs_path = "/home/eric/" + slug.replace("home-eric-", "")
    else:
        # tmp-* / 数字 hash 之类 - 跳过, 让 daemon 报错
        fs_path = None

    if fs_path:
        return {
            "id": hashlib.md5(fs_path.encode()).hexdigest(),  # workspaceStorage hash 格式
            "uri": {"fsPath": fs_path, "scheme": "file"}
        }
```

这个推断规则覆盖了你的实际项目 (enenzuo / pawcare / langchain_practice 等)。

---

## 7. 仓库目录

```
~/workspace/bettercursor/
├── BACKGROUND.md      ← 你正在读
├── sessions.csv       ← enenzuo 17 条 session 完整盘点
└── cursync/           ← Phase 1+2 实施目录 (待建)
    ├── bettercursor-bridge.py   (本地单向, dry-run 默认)
    └── bettercursor-syncd.py    (暴露 SSH 命令接口)
```

---

## 8. 时间线

| 日期 | 进展 |
|------|------|
| 2026-07-02 上午 | 发现 5 JSONL-only session, 写 cursync-import.py 骨架 |
| 2026-07-02 下午 | 确认 cursor-server 不写 SQLite, 旧模型 |
| 2026-07-02 晚 | 整理会话清单 + 旧 BACKGROUND |
| 2026-07-02 深夜 | **重大发现**: 用户实证 cursor-agent CLI 读 Layer 2, Desktop 读 Layer 3, 两层独立 |
| 2026-07-03 凌晨 | **架构反转**: CPU 实测确认 Mac 是本地 Electron + SSH 文件 (模型 A), 不是 thin client. 推翻"Linux source of truth". Mac 不开 SSH server → 选反向推送架构. |
| 2026-07-03 | **BACKGROUND 完善**: 补 CPU 实测证据 / 4 层架构细节 / 17 session 分布表 / chat_root MD5 验证 / 空草稿过滤逻辑 / Mac 双 workspaceStorage 解释 / 6 个待回答 |
| 2026-07-03 午后 | **cursaves 摸排完成**: 在 Linux 装 Callum-Ward/cursaves, 拿到 c1ea7999 snapshot. 80 agentBlobs (76 JSON + 4 protobuf tree nodes), key = SHA256(raw bytes) 验证通过. cursaves importer 第 416 行揭示: store.db 的 key 格式是 `agentKv:blob:<id>`, 与 cursor-agent CLI 完全一致. |
| 2026-07-03 午后 | **Linux adapter POC 完成**: 发现 c1ea7999 的 store.db 已有 4 blobs 但 `meta[0].latestRootBlobId = ""` (空字符串) → cursor-agent --resume 失败. 写 `adapter/fix_orphan_sessions.py` (172 行) 自动找 root 候选. **修复后 cursor-agent --resume 工作, agent 记得全部上下文** (验证问"我之前问过你什么"答"你觉得够健硕吗"). |

---

## 9. 完整流程示意 (从用户视角)

```
用户日常: Mac 上开 Cursor → 连 enenzuo (Linux via SSH) → 与 agent 对话

  Mac UI (Renderer 158% CPU) 生成消息
        ↓
  Cursor API (云) 推理
        ↓
  Mac Renderer 流式接收
        ↓
  Mac 写入 ~/Library/.../state.vscdb  ← Layer 3' 索引更新
        ↓ (通过 SSH 自动)
  Mac 写 ~/.cursor/projects/home-eric-workspace-enenzuo/
            agent-transcripts/<uuid>/<uuid>.jsonl  ← Layer 1 落到 Linux

切换到 Linux CLI:
  $ cursor-agent --resume=67f8008e-...
        ↓
  Linux cursor-agent 读 ~/.cursor/chats/c19d07070edc77b1fdcdaf0dfecaf97f/
                        67f8008e-.../store.db  ← Layer 2, 找不到!
  → "session not found" (因为 Layer 2 没有 67f8008e 的条目)

切到 Linux Desktop:
  打开 Sidebar
        ↓
  Linux Electron 读 ~/.config/Cursor/User/globalStorage/state.vscdb
                   composer.composerHeaders  ← Layer 3, 找不到 67f8008e
  → Sidebar 不显示 67f8008e

daemon 介入后:
  Linux daemon 监听 JSONL (Layer 1) 看到 67f8008e 有 11u/41a
        ↓
  daemon 推断 workspaceIdentifier (从 slug 反推)
        ↓
  daemon 写:
    - ~/.cursor/chats/<md5>/67f8008e/.../{meta.json, prompt_history.json, store.db}
    - ~/.config/Cursor/User/globalStorage/state.vscdb
        ↓
  cursor-agent --resume=67f8008e → 找到 ✓
  Linux Desktop Sidebar → 看到 ✓

Mac client 介入后:
  launchd 触发 bettercursor-sync
        ↓
  ssh linux 'bettercursor-syncd export --project=enenzuo'
        ↓
  接收 JSON 流 (含 67f8008e 等 7 条 enenzuo session)
        ↓
  解析 → 写 Mac ~/Library/.../state.vscdb
        ↓
  Mac 重启 Cursor, Sidebar 看到所有 7 条 ✓
```

---

## 10. 一句话总结

> Mac 是用户日常, Linux 是 daemon 大本营。
> Mac 通过 SSH 推给 Linux, Linux daemon 维护 Linux 各层 + 暴露 SSH 接口给 Mac 拉。
> 跨端共享的是 JSONL (Layer 1, 已被 Cursor 自动同步到 Linux workspace)。
> 跨端**不共享**的是各端 SQLite (Layer 2/3/3'), 需要 daemon 桥接。
> 约束 (你只在 1 个端活跃) 保证写并发不会冲突。

---

## 11. History Normalization (历史 commit message 中文化)

> 触发条款: AGENTS.md "提交与协作" → "关于已发布 commit message 的中文化 / 规范化重写"

**动机**: bettercursor 首个 v0.1 → v0.2 阶段恰好赶上仓库 AGENTS.md 引入
"中文 + emoji 前缀" 的提交规范。最初 6 条 commit 是规范落地之前用英文写的;
与其让后人 `git log` 时看到双轨制, 不如趁单人 / 规范定型期一次性 normalize。

**方法**: `git filter-branch -f --msg-filter /tmp/msg.sh 21227fd`
(`21227fd` 是规范化起点之前的最老 HEAD, 涵盖全部 6 条需要重写的 commit;
`7136913` AGENTS.md 提交留在范围之外不动。)

**旧的 → 新的 SHA + subject 映射**:

| old SHA | old subject | new subject |
|---|---|---|
| `f8b1626` | `Initial commit: bettercursor v0.1 working (Tauri + React + Rust)` | `✨ feat: v0.1 初始可用版本 (Tauri + React + Rust)` |
| `35e7aed` | `docs: add README.md` | `📄 docs: README.md 项目说明` |
| `3c72440` | `feat(ui): distinguish title fallback from real extracted titles` | `✨ feat(ui): 标题 fallback 与真实标题视觉区分` |
| `09051b9` | `fix(core): Layer 1 JSONL title extraction — correct path & schema` | `🐛 fix(core): Layer 1 JSONL 标题提取 — 路径 + 嵌套 schema` |
| `508d3b3` | `feat(B): load conversation bubbles from Layer 1 JSONL` | `✨ feat: 对话气泡记录加载 (Phase T3, v0.2)` |
| `21227fd` | `feat(I+II): full-content search + wired sort modes` | `✨ feat: 全文搜索 + 排序按钮接通` |

**正文 (body)**: 全部中文化 (subject + body), body 保留代码路径 / 单测名等
技术细节 (保持英文标识符)。

**约束执行**:
- 仅修改 commit message, 文件内容 (patch) 不动
- push 用 `--force-with-lease` (不是 `--force`)
- 重写后跑 `cargo test` (16/16 通过) + `pnpm exec tsc --noEmit` (clean) 确认内容未受影响

**何时失效**: 项目进入多人协作 / 达到 `v1.0` 时, 本豁免条款失效,
未来新增的已发布 commit 不再被允许重写。