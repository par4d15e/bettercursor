# bettercursor — Tauri + Rust 实施计划 (MINIMAL SCOPE → v0.2.1 增量)

> **范围演化**:
> - **v0.1** (MINIMAL SCOPE, 2026-07-03 完工) = 只读 session 查看器. 不做同步, 不做 daemon, 不做 SSH, 不做删除以外的写操作.
> - **v0.2-alpha** (2026-07-03 增量) = 加**手动单 session L2↔L3 补层 sync** (单按钮一键补齐). 不是全量 sync loop. 详见 [SYNC_DESIGN §0.5](SYNC_DESIGN.md).
> - **v0.2.1** (2026-07-04 完工) = 修 orphan (latestRootBlobId 空字符串) + 删除 session (L1 + L2, L3 跳). 详见 [SYNC_DESIGN §9](SYNC_DESIGN.md).
> - **v0.2.2+** = 对话记录展开 / 全量 sync loop / 跨设备. 详见 [SYNC_DESIGN §9](SYNC_DESIGN.md).

> 配套: [PRD.md](PRD.md) 是产品需求, [BACKGROUND.md](BACKGROUND.md) 是调研考古, [SYNC_DESIGN.md](SYNC_DESIGN.md) 是 v0.2+ 设计稿.

---

## 0. 决策记录

| 选项 | 决策 |
|------|------|
| **v0.1 产品范围** | **只读 session 查看器**. 对应 cc-switch 的"会话管理"那一屏, 删掉 cc-switch 的"添加 Provider / 删除 Provider"和"添加 MCP"等写操作. |
| **v0.2-alpha 增量范围** | 单 session L2↔L3 补层 sync. 单按钮. Cursor/cursor-agent 进程检测 + 硬锁. |
| **Tauri 版本** | **Tauri v2**. |
| **语言** | **Rust + React/TypeScript**. 单一语言, 无 sidecar. |
| **SQLite 库** | **rusqlite + r2d2**, 读模式. WAL-safe 读. |
| **前端框架** | **React 18 + Vite + TypeScript**. Zustand 状态管理. UI 库用 **shadcn/ui + Tailwind** (cc-switch 同款风格). |
| **图标** | **lucide-react** (cc-switch 同款). |
| **数据更新** | 启动时扫描 + 手动 refresh 按钮. **不监听文件变更** (无 notify crate), 用户点 refresh 重扫. |

---

## 1. cc-switch UI 1:1 还原 → bettercursor

### 1.1 左面板 (会话列表树)

| cc-switch | bettercursor |
|-----------|--------------|
| 顶部 "会话列表 61" + 工具栏 | 顶部 "会话列表 <N>" + 工具栏 (搜索 / 排序 / 刷新) |
| 根节点 "Codex" (61) | 根节点 **"Cursor"** (N) |
| 子目录 "enenzuo" (32) / "eric" (3) / "cs2_tracker" (26) | 子目录 = `project_slug` (e.g. "enenzuo" / "eric-pc" / "cs2-tracker") |
| 每条 session | 展开叶子 = CanonicalSession 行 |

### 1.2 右面板 (会话详情)

| cc-switch | bettercursor |
|-----------|--------------|
| 标题 (首条 user message 预览) | session name + 首条 user message preview |
| 时间戳 / 项目名 / rollout ID | last_updated_at / project_slug / uuid |
| `codex resume 019f1d03-354b-...` + 复制按钮 | `cursor-agent --resume <uuid>` + 复制按钮 (注: Mac 端是 Desktop, 命令不同, 见 §3) |
| "删除会话" 红色按钮 | "删除会话" 红色按钮 (Phase 2 实现) |
| "对话记录 2402" 折叠 + 消息列表 | "对话记录 <N>" + 消息列表 |
| AI 消息块 (时间戳 + 内容) | 同 |
| 右下角悬浮 action button | 同 (点 → 滚动到顶 / 复制 resume) |

### 1.3 来源标签 (goal.md #1 核心)

cc-switch 用 "Codex" 区分 provider. 我们只有 Cursor 一个 provider, 但**每个 session 在哪一端产生**是关键信息. 在每条 session 行右侧加 `<SourceBadge>`:

- 🖥 **Mac Desktop** (Layer 3' on Mac)
- ⌨️ **Linux CLI** (Layer 2 on Linux)
- 🐧 **Linux Desktop** (Layer 3 on Linux, Mode A)

样式: cc-switch 风格的小灰底 + 圆角. 颜色:
- Mac: 浅蓝 (`bg-blue-500/20 text-blue-300`)
- Linux CLI: 浅绿 (`bg-green-500/20 text-green-300`)
- Linux Desktop: 浅紫 (`bg-purple-500/20 text-purple-300`)

---

## 2. 项目结构 (最小)

```
bettercursor/
├── src-tauri/                     ← Rust 入口
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   └── src/
│       ├── main.rs                ← Tauri app entrypoint
│       ├── lib.rs                 ← bettercursor_lib::run()
│       ├── commands.rs            ← tauri::command 列表
│       ├── state.rs               ← AppState (sessions cache)
│       └── core/                  ← 纯 Rust 库
│           ├── mod.rs
│           ├── paths.rs           ← 4 层路径解析
│           ├── storage.rs         ← WAL-safe 读 SQLite
│           └── canonical.rs       ← 4 层 session 合并
├── src/                           ← React 前端
│   ├── App.tsx
│   ├── main.tsx
│   ├── index.css                  ← Tailwind
│   ├── components/
│   │   ├── SessionTree.tsx        ← 左面板
│   │   ├── SessionRow.tsx         ← 树叶子
│   │   ├── SourceBadge.tsx        ← 来源标签
│   │   ├── SessionDetail.tsx      ← 右面板容器
│   │   ├── SessionHeader.tsx      ← 标题 + 元信息 + resume 按钮
│   │   ├── MessageList.tsx        ← 对话记录
│   │   └── DeleteButton.tsx       ← 删除 (Phase 2)
│   ├── store/
│   │   └── sessionStore.ts        ← Zustand: sessions, selected, refresh
│   └── lib/
│       ├── tauri.ts               ← invoke() wrappers
│       └── types.ts               ← CanonicalSession (TS mirror)
├── bettercursor/                  ← Python 参考实现 (保留, 不进 runtime)
├── vendored/cursaves/             ← 只读参考
├── cc-switch-session.png          ← UI 参照
├── goal.md
├── PRD.md
├── BACKGROUND.md
└── TAURI_RUST_PLAN.md             ← 本文件
```

**对比原 plan**: 删掉了 `sync/`, `daemon.rs`, `watcher.rs`, `core/blob_dag.rs` (只读不需要修 root), `core/snapshot.rs` (不导入), `core/conflict.rs` (不写入就不冲突), `core/layer2.rs` / `layer3.rs` (只读用 storage.rs 直接读). **核心模块从 8 个减到 3 个**.

---

## 3. Tauri Commands (前端可调用 API)

| Command | 参数 | 返回 | 阶段 |
|---------|------|------|------|
| `list_sessions` | `cwd: Option<String>` (默认 ~) | `Vec<CanonicalSession>` | Phase 1 |
| `get_session_detail` | `uuid: String` | `SessionDetail` (含 messages) | Phase 1 |
| `get_resume_command` | `uuid: String`, `source: String` | `String` (e.g. `"cursor-agent --resume c1ea7999"`) | Phase 1 |
| `refresh_sessions` | — | `Vec<CanonicalSession>` (重扫) | Phase 1 |
| `delete_session` | `uuid: String` | `()` (删除 store.db 行 + JSONL) | Phase 2 |
| `get_provider_name` | — | `"Cursor"` (硬编码) | Phase 1 |
| `dry_run_inject_layer3` | — | — | 已迁移到 `sync_session_layer23` (v0.2-alpha) |
| `prepare_inject_layer3` | — | — | 已迁移到 `sync_session_layer23` (v0.2-alpha) |
| `inspect_prepared_layer3` | — | — | 已迁移到 `sync_session_layer23` (v0.2-alpha) |
| `sync_session_layer23` | `uuid: String`, `cwd: Option<String>` | `core::sync::SyncReport` (单 session L2/L3 补层) | **v0.2-alpha ✅** |
| `fix_orphans` | — | `core::sync::FixOrphansReport` (扫所有 store.db, 修 root 空字符串) | **v0.2.1 ✅** |
| `delete_session` | `uuid: String`, `cwd: Option<String>`, `project_slug: Option<String>` | `core::sync::DeleteReport` (L1 + L2 直 rm, L3 跳) | **v0.2.1 ✅** |

**为什么没有 `set_auto_sync` / `sync_now` / `import_snapshot`**: 不在 v0.1 / v0.2-alpha / v0.2.1 范围内. 见 [SYNC_DESIGN §4-§5](SYNC_DESIGN.md) (v0.2.3 待做).

---

## 4. CanonicalSession (TS mirror)

```typescript
// src/lib/types.ts
export type SourceLayer = "mac" | "linux_cli" | "linux_desktop";

export interface SourceInfo {
  last_seen_at: number;        // ms epoch
  layer: "3'" | "2" | "3";
  path: string;                // for debugging
}

export interface CanonicalSession {
  uuid: string;
  project_slug: string;
  project_path: string;
  chat_root: string;            // md5(cwd)
  name: string;
  last_updated_at: number;
  bubble_count: number;
  is_empty_draft: boolean;
  sources: Partial<Record<SourceLayer, SourceInfo>>;
  first_user_message_preview: string;
  files_referenced: string[];
}
```

---

## 5. 实施阶段 (新, 接 PRD §7 Phase 0)

### Phase T0 — Tauri 项目骨架 (半天)

| 任务 | 验收 |
|------|------|
| 验证 Rust 工具链: `rustc --version` ≥ 1.75, Node ≥ 18 | 命令成功 |
| `cargo install tauri-cli --version "^2.0"` | 装好 |
| 手工写 `src-tauri/Cargo.toml` + `tauri.conf.json` + `src/main.rs` + `src/lib.rs` | `cargo tauri dev` 跑出空窗口 |
| 写 `src/App.tsx` 显示 "会话管理" 标题 | 标题显示 |
| 装 Tailwind + shadcn/ui (或直接抄 cc-switch CSS) | 暗色主题生效 |

### Phase T1 — Rust 只读核心 (1.5 天)

| 任务 | 验收 |
|------|------|
| 端口 `paths.py` → `core/paths.rs` | 单元测试覆盖 4 层 |
| 端口 `storage.py` (读函数) → `core/storage.rs` | WAL-safe 读通过 |
| 新写 `core/canonical.rs` (扫 4 层 + 合并) | 17 session 实测结果与 Python 一致 |
| 写 `commands::list_sessions` / `get_session_detail` | `cargo test` 通过 |

**关键简化**: 写函数 (`write_blobs_batch`, `import_snapshot_to_layer2`) **暂不移植**, 因为本阶段只读. 写函数在 Phase 2 (delete) 才需要.

### Phase T2 — UI 主面板 (2 天)

| 任务 | 验收 |
|------|------|
| Tailwind + shadcn/ui 装好 | 暗色 + 圆角风格跟 cc-switch 一致 |
| `<SessionTree>` 左面板 | 根节点 "Cursor" + 子目录 = project_slug + 叶子 = session |
| `<SourceBadge>` 3 种颜色 | 视觉上一眼区分 |
| `<SessionDetail>` 右面板 | 标题 + 元信息 + resume 命令 + 复制按钮 |
| `<MessageList>` 展开对话记录 | cc-switch 同款样式 |
| 搜索框过滤 | 输入字符实时过滤 |
| 刷新按钮: 调 `refresh_sessions` | 重扫并更新 UI |
| 选中高亮 | 点行高亮, 详情同步切换 |

### Phase T3 — 跨端 (可选, v0.2)

> 用户没要求, 暂搁置. 留接口.

| 任务 | 验收 |
|------|------|
| Mac 端单独装 (Tauri build 一次) | Mac 上能扫 Layer 3' |
| `get_resume_command` 区分 source: mac 返回 `open -a Cursor`, linux_cli 返回 `cursor-agent --resume ...`, linux_desktop 同 mac | 复制按钮拿到的命令对 |

### Phase T4 — 删除 (Phase 2 推迟项, 用户没要求)

> 暂不实现. goal.md 只说"查看", 没说"删除". cc-switch 有删, 但 bettercursor 可以不带.

---

## 6. Rust 依赖 (Cargo.toml)

```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-fs = "2"            # 读 ~/.cursor (需要 allowlist ~/.cursor/**)
tauri-plugin-shell = "2"          # 复制 resume 命令到剪贴板 (或用 tauri-plugin-clipboard-manager)
tauri-plugin-clipboard-manager = "2"  # 写剪贴板
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
r2d2 = "0.8"
r2d2_sqlite = "0.25"
dirs = "5"
home = "0.5"
sha2 = "0.10"
hex = "0.4"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
thiserror = "1"
log = "0.4"
env_logger = "0.11"
walkdir = "2"

[dev-dependencies]
tempfile = "3"
```

**对比原 plan**: 删掉了 `tokio`, `notify`, `flate2`, `prost` (snapshot 不需要). 体积估计 **5-8 MB** (vs 原 8-12 MB).

---

## 7. 关键文件 (写代码前先打开的)

- [cc-switch-session.png](cc-switch-session.png) — UI 1:1 参照
- [PRD.md §4.1](PRD.md) — CanonicalSession 字段定义
- [PRD.md §4.2](PRD.md) — 4 层存储路径表
- [vendored/cursaves/cursor_saves/paths.py](../vendored/cursaves/cursor_saves/paths.py) — 路径解析参考
- [vendored/cursaves/cursor_saves/db.py](../vendored/cursaves/cursor_saves/db.py) — WAL-safe SQLite 读参考
- [bettercursor/paths.py](paths.py) — Python 端口源 (182 行)
- [bettercursor/storage.py](storage.py) — Python 端口源 (254 行, 只读部分)
- [vendored/cursaves/cursor_saves/watch.py](../vendored/cursaves/cursor_saves/watch.py) — 4 层扫表参考 (Python 端用过的 scan 函数)

---

## 8. 风险与缓解

| 风险 | 缓解 |
|------|------|
| Tauri v2 在 Linux 上 WebKitGTK 编译慢 | `cargo tauri dev` 第一次 5-10 分钟, 之后增量快. CI 缓存. |
| cc-switch 风格 CSS 抄不对 | 直接 `view-source:` 看 cc-switch 的类名, 或用 shadcn 替代品 |
| 同 UUID 多源合并时谁先谁后 | `last_updated_at` 排序, 最新在上. 已实现逻辑 (Python 端验证过). |
| JSONL 文件很大 (10MB+) 解析慢 | 启动时只读首条 user message 做 preview, 不全量 parse. 详情面板按需加载. |
| Mac / Linux UI 略有差异 (字体 / 间距) | 用 `system-ui` 字体栈, Tailwind 默认响应式 |

---

## 9. 不在范围内 (明确)

- ❌ 同步任何端之间的 session
- ❌ 监听文件变更 (notify)
- ❌ 后台 daemon
- ❌ SSH 服务 / 跨设备
- ❌ 修 orphan session (`fix_latest_root`)
- ❌ 写 store.db (除 delete)
- ❌ 写 state.vscdb (除 delete)
- ❌ 写 snapshot 文件
- ❌ 实时刷新 (用户点 refresh 才扫)

如果以后要做, 增量加, 不破坏现有 UI.

---

## 10. 退出策略

- **如果 Tauri 装不上**: 改 Electron + TS, 前端代码 90% 复用 (React 不变)
- **如果 Rust port 慢**: storage.rs 临时用 Python sidecar (子进程跑 `python3 -c "import json; ..."`), 后续替换
- **如果用户改主意加 sync**: storage.rs 加写函数, 新增 sync 模块, 不动前端
