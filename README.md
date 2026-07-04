# bettercursor

> 本地 **Cursor** 会话查看器 (只读). **Tauri 2 + React 19 + Rust**, UI 范式借鉴 [cc-switch](https://github.com/farion1231/cc-switch).

![status](https://img.shields.io/badge/status-v0.2.5-success)
![platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue)
![stack](https://img.shields.io/badge/Tauri-2-orange)
![language](https://img.shields.io/badge/Rust-1.77%2B-orange)
![i18n](https://img.shields.io/badge/i18n-zh--CN%20%7C%20en-green)

## 它是什么

`bettercursor` 是一个桌面应用, 用来 **查看** 本机 Cursor IDE 在磁盘上存储的所有 AI 会话. 它扫描
`~/.config/Cursor` (Linux) / `~/Library/Application Support/Cursor` (macOS) 下三层 SQLite + JSONL
数据, 跨层去重合并后呈现给用户.

设计目标:
- **v0.2.1+ 可写 (受控)** — 仅后端命令明确允许时才写 (`sync_session_layer23` / `fix_orphans` / `delete_session`), 仍有 v0.1 阶段不可写越权
- **借鉴 cc-switch UI** — 左侧项目分组树 + 右侧会话详情的范式
- **与 Python 守护进程版本字节级一致** — MD5 `chat_root` 实现 parity 测试通过

## 功能状态

### v0.2.5 (✅ 当前, 2026-07-04 完工)

- [x] **跨平台打包**: Linux `.deb` / `.AppImage` + macOS 未签名 `.dmg` (Mac 10.15+) + Windows `.msi` / `.exe` (NSIS), 全部通过 GitHub Actions 矩阵自动 build ([`release.yml`](.github/workflows/release.yml))
- [x] **i18n (zh-CN / en)**: react-i18next + `src/locales/{zh-CN,en}.json` (~110 条 UI 字符串) + `<LanguageSwitcher>` 头部 `<select>` + localStorage 持久化 (`i18nextLng`)
- [x] 三件套 version bump: `package.json` / `Cargo.toml` / `tauri.conf.json` 都升到 `0.2.5`, `productName: "BetterCursor"` (PascalCase for Mac `.app`)
- [x] 后台 sync loop 收尾 (v0.2.3): `<SyncNowButton>` (立即扫描) + `<SyncStatusBadge>` ("● 自动同步 · Xs 前", 1Hz tick + 5s 后端 poll)
- [x] 对话记录展开 (v0.2.2): L1+L2+L3 三路合并 + `<MessageList>` 薄包装
- [x] 修 orphan + 删 session (v0.2.1): `<dialog>` 原生确认
- [x] 启动时扫描 3 层 Cursor 存储 (Layer 1 JSONL / Layer 2 `store.db` / Layer 3 `state.vscdb`)
- [x] 跨层去重合并, 项目分组, 按会话名 / 项目 / 内容 / UUID 全文搜索
- [x] MD5 `chat_root` 与 Python 守护进程字节级一致

### v0.2.6 / v0.3 (规划, 详见 [SYNC_DESIGN.md](SYNC_DESIGN.md))

- [ ] 跨设备 sync (Tailscale / SSH-rsync) — §4 transport trait 初版
- [ ] `~/.bettercursor/unified.db` (snapshot codec + Conflict enum 大版本)
- [ ] outbox flush + 5-way 分类 conflict UI
- [ ] T3/T4/T5 adapter: git / S3 / Tailscale

## 技术栈

| 层 | 选型 |
|---|---|
| Shell | [Tauri 2](https://tauri.app) |
| Frontend | React 19 · TypeScript · Vite · Tailwind CSS · Zustand 5 · Lucide icons |
| Backend | Rust (1.77+, `rusqlite` + `r2d2`, WAL-safe readers) |
| IPC | Tauri command + event (`list_sessions`, `refresh_sessions`, `get_resume_command`, `platform_info`) |

> **为什么不是 Electron**: Tauri 用系统 WebView, 二进制小 (≈15 MB), 后端 Rust 可直接复用 SQL 解析逻辑.

## 下载安装

每个 git tag (`v*.*.*`) 都触发 [`.github/workflows/release.yml`](.github/workflows/release.yml)
三平台矩阵 build, 产物在 [Releases](../../releases) 页:

### Linux

```bash
# Debian / Ubuntu (.deb, 含 libwebkit2gtk-4.1 / libgtk-3 / libayatana-appindicator3)
sudo dpkg -i BetterCursor_0.2.5_amd64.deb
sudo apt-get install -f   # 补依赖 (如 dpkg 报缺包)

# 便携 AppImage (无需安装, 但首次 build 需联网下载 linuxdeploy 二进制)
chmod +x BetterCursor_0.2.5_amd64.AppImage
./BetterCursor_0.2.5_amd64.AppImage
```

### macOS

1. 下载 `BetterCursor_0.2.5_aarch64.dmg` (Apple Silicon) 或 `BetterCursor_0.2.5_x64.dmg` (Intel — pending release fix, see issue tracker)
2. 双击挂载, 把 `BetterCursor.app` 拖进 `/Applications`
3. **未签名 dmg 跳过 Gatekeeper** (一次性, 比"右键打开"更彻底):

   ```bash
   xattr -dr com.apple.quarantine /Applications/BetterCursor.app
   ```

   `com.apple.quarantine` 是 Finder 给从 internet 下载的 dmg 应用打的扩展属性, 留着它 Gatekeeper 每次双击都会拦截. `xattr -dr` 递归删掉整个 app bundle 下的所有 quarantine 标记 (包括嵌套 binary / framework), 之后双击就跟装 App Store 一样了.

   兜底 (上一步不生效时): `右键 BetterCursor.app → 打开方式 → 打开` 同样能解锁, 但**每个新下载的 app 都要做一次**.

   全局 sweep (清 /Applications 下所有 quarantined app):

   ```bash
   find /Applications -name "*.app" -exec xattr -dr com.apple.quarantine {} \; 2>/dev/null
   ```

### Windows

```powershell
# .msi (MSI installer, 适合企业部署)
msiexec /i BetterCursor_0.2.5_x64_en-US.msi

# 或 .exe (NSIS, 适合个人)
.\BetterCursor_0.2.5_x64-setup.exe
```

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
v0.2.5 (✅ now)  跨平台打包 · i18n · 后台 sync · 对话记录 · 修复 orphan · 删除
v0.2.6 (next)   跨设备 sync (Transport trait 初版)
v0.3.0 (later)  ~/.bettercursor/unified.db · snapshot codec · Conflict UI
```

## 致谢

- UI 范式来自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)
- 旧版 Python 守护进程 (`bettercursor/`, `adapter/`) 提供了解析算法参考
- `vendored/cursaves/` 是上游 Cursor 解析库的快照

---

> 当前为个人早期项目. v0.2 阶段计划开公开仓库, 收 issue 和 PR.
