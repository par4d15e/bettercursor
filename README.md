# bettercursor

> 本地 **Cursor** 会话查看器 (只读). **Tauri 2 + React 19 + Rust**, UI 范式借鉴 [cc-switch](https://github.com/farion1231/cc-switch).

![status](https://img.shields.io/badge/status-v0.1%20working-success)
![platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS-blue)
![stack](https://img.shields.io/badge/Tauri-2-orange)
![language](https://img.shields.io/badge/Rust-1.77%2B-orange)

## 它是什么

`bettercursor` 是一个桌面应用, 用来 **查看** 本机 Cursor IDE 在磁盘上存储的所有 AI 会话. 它扫描
`~/.config/Cursor` (Linux) / `~/Library/Application Support/Cursor` (macOS) 下三层 SQLite + JSONL
数据, 跨层去重合并后呈现给用户.

设计目标:
- **只读** — v0.1 阶段不写任何文件, 避免污染 Cursor 自身的工作目录
- **借鉴 cc-switch UI** — 左侧项目分组树 + 右侧会话详情的范式
- **与 Python 守护进程版本字节级一致** — MD5 `chat_root` 实现 parity 测试通过

## 功能状态

### v0.1 (✅ 当前)

- [x] 启动时扫描 3 层 Cursor 存储 (Layer 1 JSONL / Layer 2 `store.db` / Layer 3 `state.vscdb`)
- [x] 跨层去重合并: 同一对话在多处存储只出现一次
- [x] 项目分组 + 会话名 + 来源 badge (`mac` / `linux_cli` / `linux_desktop`)
- [x] 手动刷新按钮, 立即重新扫描
- [x] 按会话名 / 项目 / 内容预览 / UUID 的全文搜索
- [x] 会话详情面板: 元数据 + 复制 resume 命令 + 来源展示
- [x] MD5 `chat_root` 与 Python 守护进程字节级一致 (`tests/test_layer2_import.py`)
- [x] 首次实跑扫到 **37 个 session** (Linux desktop + Linux CLI + macOS 来源混合)
- [x] Linux Wayland 兼容性验证通过 (经过 webkit2gtk + Mutter 实测)

### v0.2 / v0.3 (规划, 详见 [SYNC_DESIGN.md](SYNC_DESIGN.md))

- [ ] 对话气泡记录加载 (Layer 1 JSONL 完整解析)
- [ ] 复制 resume 命令后自动 spawn Cursor
- [ ] 删除会话 (引入写权限)
- [ ] 本地自动同步 (Snapshot codec + 后台 loop)
- [ ] 跨设备同步 (云端 / P2P 节点)

## 技术栈

| 层 | 选型 |
|---|---|
| Shell | [Tauri 2](https://tauri.app) |
| Frontend | React 19 · TypeScript · Vite · Tailwind CSS · Zustand 5 · Lucide icons |
| Backend | Rust (1.77+, `rusqlite` + `r2d2`, WAL-safe readers) |
| IPC | Tauri command + event (`list_sessions`, `refresh_sessions`, `get_resume_command`, `platform_info`) |

> **为什么不是 Electron**: Tauri 用系统 WebView, 二进制小 (≈15 MB), 后端 Rust 可直接复用 SQL 解析逻辑.

## 快速开始

### 前置依赖

- **Node 20+** + pnpm 9+
- **Rust 1.77+** (`rustup install stable`)
- **Linux**: `webkit2gtk-4.1`, `libsoup-3.0`, `libgtk-3`, `libjavascriptcoregtk-4.1`,
  可选 `xdg-desktop-portal-gnome`
- **macOS**: Xcode Command Line Tools

### 安装与运行

```bash
git clone https://github.com/par4d15e/bettercursor.git
cd bettercursor
pnpm install

# 开发模式 (HMR + WebKit devtools 可用)
pnpm tauri dev

# 生产构建
pnpm tauri build
```

启动后会自动展开一个 1280×800 的窗口, 先在后台线程里异步扫一遍 Cursor 存储, 出 37 条左右会话.

### Wayland 用户注意

部分 compositor 在 WebKitGTK 下需要降级环境变量:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 \
LIBGL_ALWAYS_SOFTWARE=1 \
pnpm tauri dev
```

## 项目结构

```
bettercursor/
├── src/                   # React + TS frontend
│   ├── components/        # SessionTree, SessionDetail, SourceBadge
│   ├── store/             # Zustand store + selectors
│   ├── lib/               # tauri.ts (IPC wrapper), types.ts
│   ├── App.tsx · main.tsx · index.css
├── src-tauri/             # Rust 后端
│   ├── src/
│   │   ├── core/
│   │   │   ├── paths.rs       # 4-layer 路径解析
│   │   │   ├── storage.rs     # WAL-safe SQLite reader
│   │   │   ├── canonical.rs   # 三层合并
│   │   ├── lib.rs             # Tauri commands + setup
│   │   ├── main.rs
│   ├── capabilities/          # default.json (ACL 白名单)
│   ├── icons/                 # Tauri bundle icons
│   ├── tauri.conf.json
│   ├── Cargo.toml · Cargo.lock
├── tests/                 # Python 兼容性测试 (parity check)
├── bettercursor/, adapter/# 旧 Python 守护进程代码 (存档)
├── vendored/              # 上游 Cursor 解析库 (子仓库)
├── PRD.md · SYNC_DESIGN.md · TAURI_RUST_PLAN.md · BACKGROUND.md · goal.md
```

## 架构概览

会话读取分 **三层**:

| 层 | 存储 | 路径 (Linux) | 角色 |
|---|---|---|---|
| **L1** | JSONL | `<workspaceStorage>/<chat_root>/<composer>/<session>.jsonl` | 最新, Cursor CLI 主存 |
| **L2** | SQLite `ItemTable` | `<…>/state.vscdb` (aiDiskKV) | 编辑器内缓存 |
| **L3** | SQLite `cursorDiskKV` | `<…>/state.vscdb` (cursorDiskKV) | 编辑器元数据 |

Rust 端 (`src-tauri/src/core/`) 负责:
1. **`paths.rs`** — 解析 cursor user dir / chat_root MD5 等
2. **`storage.rs`** — WAL-safe 读: `tempfile::tempdir()` 拷贝 + `PRAGMA wal_checkpoint(TRUNCATE)` 后只读打开
3. **`canonical.rs`** — 跨层合并, 输出统一的 `CanonicalSession`

四个 Tauri command 暴露给前端:

| 命令 | 入参 | 返回 |
|---|---|---|
| `list_sessions` | — | 当前缓存的全部 session |
| `refresh_sessions` | — | `usize`, 同时发 `sessions-updated` 事件 |
| `get_resume_command` | `uuid`, `source` | `open -a Cursor --args --resume <uuid>` 或 `cursor-agent --resume <uuid>` |
| `platform_info` | — | `<os>: <cursor_user_dir>` (调试用) |

## 踩坑记录

### 1. React 19 + Zustand 5 无限 re-render

`useShallow((s) => derived(s))` 看似处理了引用稳定性, 但当 derived 函数里有 `[...arr].sort(...)` 时, 浅比较只看一层, 内层 array ref 不等就判定失败, 反复触发 React 拉闸 `Maximum update depth exceeded`.

**修法**: 把 derived 移出 selector, 在组件层用 `useMemo([...])` memoize.

```ts
// ❌ 不要
useStore(useShallow((s) => groupByProject(s.items)))

// ✅
const items = useStore((s) => s.items)
const grouped = useMemo(() => groupByProject(items), [items])
```

### 2. Tauri capabilities ACL 默认最小

不写明 plugin-specific 权限, 前端调 `invoke('plugin:foo|bar')` 会被拒, 表现为 `undefined.invoke` TypeError.

```json
{
  "permissions": [
    "core:default",
    "opener:default",
    "fs:default",
    "clipboard-manager:allow-write-text",
    "clipboard-manager:allow-read-text"
  ]
}
```

### 3. WebKitGTK devtools 默认不打开

必须在 `Cargo.toml` 里 opt-in, 否则右键菜单只有 Reload 没有 Inspect:

```toml
tauri = { version = "2", features = ["devtools"] }
```

### 4. WebKitGTK Wayland 黑屏

部分 compositor (Mutter / Hyprland 等) 在 GPU 合成路径上挂掉. 经验方案:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 \
WEBKIT_DISABLE_COMPOSITING_MODE=1 \
LIBGL_ALWAYS_SOFTWARE=1 \
pnpm tauri dev
```

## 文档

| 文件 | 内容 |
|---|---|
| [PRD.md](PRD.md) | 产品需求 v0.1 功能矩阵 + 验收标准 |
| [SYNC_DESIGN.md](SYNC_DESIGN.md) | v0.2+ 同步功能设计文档 |
| [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) | Python → Rust 模块映射 + Cargo 依赖清单 |
| [BACKGROUND.md](BACKGROUND.md) | 项目历程 (Python 守护进程 → Tauri 重构) |
| [goal.md](goal.md) | 原始 brief |

## 路线图

```
v0.1 (✅ now)  只读查看 · 复制 resume
v0.2 (next)   对话内容 · 删除 · 本地自动同步
v0.3 (later)  跨设备同步
```

## 致谢

- UI 范式来自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)
- 旧版 Python 守护进程 (`bettercursor/`, `adapter/`) 提供了解析算法参考
- `vendored/cursaves/` 是上游 Cursor 解析库的快照

---

> 当前为个人早期项目. v0.2 阶段计划开公开仓库, 收 issue 和 PR.
