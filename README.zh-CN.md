# bettercursor

> 本地 **Cursor** 会话查看器 (只读). **Tauri 2 + React 19 + Rust**, UI 范式借鉴 [cc-switch](https://github.com/farion1231/cc-switch).
>
> 🌐 [English](README.md) · [简体中文](README.zh-CN.md)

![status](https://img.shields.io/badge/status-v0.3.4-success)
![platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue)
![stack](https://img.shields.io/badge/Tauri-2-orange)
![language](https://img.shields.io/badge/Rust-1.77%2B-orange)
![i18n](https://img.shields.io/badge/i18n-zh--CN%20%7C%20en-green)
![sync](https://img.shields.io/badge/sync-Transport%20trait%20v1-purple)

## 它是什么

`bettercursor` 是一个桌面应用, 用来 **查看** 本机 Cursor IDE 在磁盘上存储的所有 AI 会话. 它扫描 `~/.config/Cursor` (Linux) / `~/Library/Application Support/Cursor` (macOS) 下三层 SQLite + JSONL 数据, 跨层去重合并后呈现给用户.

设计目标:
- **v0.2.1+ 可写 (受控)** — 仅后端命令明确允许时才写 (`sync_session_layer23` / `fix_orphans` / `delete_session`), v0.1 阶段不写越权
- **借鉴 cc-switch UI** — 左侧项目分组树 + 右侧会话详情的范式
- **与 Python 守护进程版本字节级一致** — MD5 `chat_root` 实现 parity 测试通过

## 功能状态

### v0.3.4 (✅ 当前, 2026-07-05 完工)

- [x] **L2→L3 bubble 富化** — `layer2_messages` 遍历 CLI `store.db` DAG, 写 `bubbleId` 前用 L2 完整 assistant 文本替换 L1 `[REDACTED]` stub
- [x] **用户图片附件** — L2 `image` blob 解码为 user bubble 的 `images[]` data URL
- [x] **重补检测** — CLI 信封 / `[REDACTED]` / 缺图时自动触发 Layer 3 重写
- [x] **补 Layer 3 操作规范** — 见 [SYNC_DESIGN §0.5](SYNC_DESIGN.md)

### v0.3.2 (2026-07-05 完工)

- [x] **`<SettingsDialog>`** — 侧栏头部齿轮入口, 整合界面语言 (`<LanguageSwitcher>`)、跨设备同步 (`<SyncPeersPanel>`)、冲突处理 (`<ConflictResolvePanel>`)
- [x] **i18n 修复** — 合并 locale JSON 重复 `sync` 键 (同步状态不再裸露 `sync.autoSync`)
- [x] **暗色语言切换** — 分段按钮替代原生 `<select>`; 根节点 `color-scheme: dark`
- [x] **侧栏精简** — 头部显示产品名 `BetterCursor`; 工具栏「全部折叠/展开」; 移除无效返回按钮
- [x] **冲突文案** — 中性表述 + 有待处理冲突时设置按钮显示角标

### v0.3.1 (2026-07-05 完工)

- [x] **LAN 跨设备同步** — mDNS 发现、6 位配对、trusted peers、outbox、后台 sync loop
- [x] **`<SyncPeersDialog>` / `<ConflictResolveDialog>`** — v0.3.1 独立弹窗; **v0.3.2 起迁入 `<SettingsDialog>`**

### v0.3.0 (2026-07-05 完工)

- [x] **`~/.bettercursor/unified.db`** (PR-1): 8 表 + FTS5 + `rebuild_from_cursor_state` + archive / conflicts / sync_runs
- [x] **pre-PR-2 读路径补全**: L3 bubble 完整文本 / Cursor 3.0+ session discovery / timestamp gaps / cursor-history parity fixtures
- [x] **snapshot codec v4** (`core/snapshot.rs`): bubbles + `ts_ms` 映射; push 仍用 8-field `snapshot_meta`
- [x] **Conflict 5-way** (`core/conflict.rs`): classify / bubble_diff / auto_merge; `transport_pull` 写回 unified.db
- [x] **Transport async** (`tokio` + `async-trait`); Tauri 命令内部 `block_on`, 前端签名不变
- [x] **agentKv 最小切片**: `write_layer3` 复制 `conversationState` 引用的 agent blob
- [x] **126 Rust 单测** (`cargo test --lib`)

查询 unified.db 示例:

```bash
sqlite3 ~/.bettercursor/unified.db "SELECT uuid, bubble_count, content_hash FROM sessions LIMIT 5;"
```

### v0.2.6 (2026-07-04 完工)

- [x] **跨设备 sync — Transport trait 初版**: `core::transport::Transport` trait (4 方法: `push` / `pull` / `list_remote` / `endpoint_id`, **同步签名** — 有意识偏离 [SYNC_DESIGN §4.4](SYNC_DESIGN.md#4-transport-trait) 的 `async_trait`, v0.3.0 上 outbox 时再迁). 一个 impl: `SshRsyncTransport` (T2), 调系统 `ssh` / `rsync` (0 新 Cargo dep, 无 tokio, 无 russh)
- [x] **最小 v0.2.6 snapshot 载体**: `SessionSnapshot` (8 个 metadata 字段 — uuid / `last_updated_at_ms` / host / `project_slug` / `project_path` / `source_path` / `text_preview` 截 280 字符 / `bubble_count`). 不含 bubbles / blobs — 那是 v0.3.0 unified.db 的活
- [x] **Peer 配置文件**: `~/.bettercursor/transports.json` (跟 `config.json` 分开). 原子 save (`*.tmp` + rename)
- [x] **4 个 Tauri 命令**: `transport_list_peers` / `transport_test` / `transport_push` / `transport_pull`. 配套 4 个 typed IPC wrapper 在 `src/lib/tauri.ts` (`PeerSummary` / `TestReport` / `PushReport` / `PullReport` / `RemoteSession`)
- [x] **暂无 UI** — 用法靠 `invoke('transport_*')` + 手动编 `transports.json`. SyncPeersDialog 是 v0.3.0 的事
- [x] **20 个 Rust 单元测试** — snapshot codec / config serde / `ssh_cmd` 安全 flag / push failure stderr 等. 配套 `tests/fixtures/fake-{ssh,rsync}.sh` mock 二进制, CI 不依赖真 SSH peer
- [x] **v0.2.6 housekeeping** (一起打包发布): CI matrix 加 `macos-13` (Intel x64 dmg 跟 Apple Silicon dmg 一起出) + Node 20 → 22 + vitest 2 + jsdom 25 + `@testing-library/react` 16 + 15 case 测 `<SyncStatusBadge>` / `<BrokenBadge>` i18n-aware fallback. 零业务代码改动

### v0.2.5 (2026-07-04 完工)

- [x] **跨平台打包**: Linux `.deb` / `.AppImage` + macOS 未签名 `.dmg` (Mac 10.15+, Apple Silicon) + Windows `.msi` / `.exe` (NSIS), 全部通过 GitHub Actions 矩阵自动 build ([`release.yml`](.github/workflows/release.yml))
- [x] **i18n (zh-CN / en)**: react-i18next + `src/locales/{zh-CN,en}.json` (~110 条 UI 字符串) + `<LanguageSwitcher>` 头部 `<select>` + localStorage 持久化 (`i18nextLng`)
- [x] 三件套 version bump: `package.json` / `Cargo.toml` / `tauri.conf.json` 都升到 `0.2.5`, `productName: "BetterCursor"` (PascalCase for Mac `.app`)
- [x] 后台 sync loop 收尾 (v0.2.3): `<SyncNowButton>` (立即扫描) + `<SyncStatusBadge>` ("● 自动同步 · Xs 前", 1Hz tick + 5s 后端 poll)
- [x] 对话记录展开 (v0.2.2): L1+L2+L3 三路合并 + `<MessageList>` 薄包装
- [x] 修 orphan + 删 session (v0.2.1): `<dialog>` 原生确认
- [x] 启动时扫描 3 层 Cursor 存储 (Layer 1 JSONL / Layer 2 `store.db` / Layer 3 `state.vscdb`)
- [x] 跨层去重合并, 项目分组, 按会话名 / 项目 / 内容 / UUID 全文搜索
- [x] MD5 `chat_root` 与 Python 守护进程字节级一致

### v0.3.2+ (规划, 详见 [SYNC_DESIGN.md](SYNC_DESIGN.md))

- [ ] T3/T4/T5 adapter: git / S3 / Tailscale

## 下载安装

每个 git tag (`v*.*.*`) 都触发 [`.github/workflows/release.yml`](.github/workflows/release.yml) 三平台矩阵 build, 产物在 [Releases](../../releases) 页:

### Linux

```bash
# Debian / Ubuntu (.deb, 含 libwebkit2gtk-4.1 / libgtk-3 / libayatana-appindicator3)
sudo dpkg -i BetterCursor_0.2.6_amd64.deb
sudo apt-get install -f   # 补依赖 (如 dpkg 报缺包)

# 便携 AppImage (无需安装, 但首次 build 需联网下载 linuxdeploy 二进制)
chmod +x BetterCursor_0.2.6_amd64.AppImage
./BetterCursor_0.2.6_amd64.AppImage
```

### macOS

1. 下载 `BetterCursor_0.2.6_aarch64.dmg` (Apple Silicon) **或** `BetterCursor_0.2.6_x64.dmg` (Intel). 两者都是未签名 dmg, 由 CI matrix 的 `macos-latest` + `macos-13` 两个 entry 一起 build
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
msiexec /i BetterCursor_0.2.6_x64_en-US.msi

# 或 .exe (NSIS, 适合个人)
.\BetterCursor_0.2.6_x64-setup.exe
```

## 快速开始 (从源码构建)

### 前置依赖

- **Node 20+** + pnpm 9+ (lockfileVersion 是 `9.0`)
- **Rust 1.77+** (`rustup install stable`)
- **Linux**: `webkit2gtk-4.1`, `libsoup-3.0`, `libgtk-3`, `libjavascriptcoregtk-4.1`, 可选 `xdg-desktop-portal-gnome`
- **macOS**: Xcode Command Line Tools
- **Windows**: WebView2 runtime (Win 11 预装)

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

启动后会自动展开一个 1280×800 的窗口, 先在后台线程里异步扫一遍 Cursor 存储, 出 37 条左右会话 (典型配置: Linux desktop + Linux CLI + macOS 来源混合).

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
│   ├── components/        # SessionTree, SessionDetail, SourceBadge, ...
│   ├── store/             # Zustand store + selectors
│   ├── lib/               # tauri.ts (IPC wrapper), types.ts
│   ├── i18n/              # i18next init (zh-CN, en)
│   ├── locales/           # zh-CN.json / en.json
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
├── PRD.md · SYNC_DESIGN.md · AGENTS.md · docs/
└── .github/workflows/     # release.yml (3-OS matrix)
```

## 架构概览

会话读取分 **三层** (UUID 身份模型见 [SYNC_DESIGN.md §2.5 Q6](SYNC_DESIGN.md)):

| 层 | 存储 | 路径 (Linux) | 角色 |
|---|---|---|---|
| **L1** | JSONL | `~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl` |  transcript; CLI 与 Desktop 都会写; **有效 CLI 会话时与 L2 同 uuid** |
| **L2** | SQLite | `~/.cursor/chats/<md5(cwd)>/<uuid>/store.db` | **仅 CLI** (`cursor-agent`) |
| **L3** | SQLite KV | `~/.config/Cursor/User/globalStorage/state.vscdb` (`cursorDiskKV` + 各 workspace `state.vscdb`) | **Desktop** composer 索引与 bubble 正文 |

### 补 Layer 3 (CLI → Desktop Sidebar)

**必须先完全退出 Cursor Desktop**, 再在 bettercursor 对目标 CLI 会话执行 **补 Layer 2/3**, 然后重启 Cursor. v0.3.4+ 会检测 CLI 信封、`[REDACTED]`、缺图并自动重写 stub bubble. 完整铁律见 [SYNC_DESIGN §0.5](SYNC_DESIGN.md).

Rust 端 (`src-tauri/src/core/`) 负责:
1. **`paths.rs`** — 解析 cursor user dir / chat_root MD5 等
2. **`storage.rs`** — WAL-safe 读: `tempfile::tempdir()` 拷贝 + `PRAGMA wal_checkpoint(TRUNCATE)` 后只读打开
3. **`canonical.rs`** — 跨层合并, 输出统一的 `CanonicalSession`

四个 Tauri command 暴露给前端:

| 命令 | 入参 | 返回 |
|---|---|---|
| `list_sessions` | — | 当前缓存的全部 session |
| `sync_now` | — | `usize`, 同时发 `sessions-updated` 事件 |
| `get_conversation` | `uuid` | 解析后的 `Conversation` + 合并气泡 + source_path |
| `get_resume_command` | `uuid`, `source` | `open -a Cursor --args --resume <uuid>` 或 `cursor-agent --resume <uuid>` |
| `sync_session_layer23` | `uuid`, `cwd` | `SyncReport` (wrote_layer2 / wrote_layer3 / skipped / duration_ms) |
| `fix_orphans` | — | `FixOrphansReport` (scanned / fixed / skipped, 自动备份 store.db) |
| `delete_session` | `uuid`, `cwd`, `slug` | `DeleteReport` (cursor_running / removed_l1 / removed_l2 / skipped_*) |
| `watcher_status` | — | `{ active, dirs[], last_scan_at_ms }` |
| `platform_info` | — | `<os>: <cursor_user_dir>` (调试用) |
| `transport_list_peers` | — | `PeerSummary[]` (从 `~/.bettercursor/transports.json`) |
| `transport_test` | `peerId` | `TestReport` (ok / latency_ms / error?) |
| `transport_push` | `uuid`, `peerId` | `PushReport` (uuid / bytes_written / duration_ms) |
| `transport_pull` | `peerId`, `sinceMs?` | `PullReport` (peer_id / count / snapshots[]) |

## 跨设备 sync (v0.2.6)

v0.2.6 落地了 **Transport trait 初版**. 先在 `~/.bettercursor/transports.json` 里配一个或多个 peer:

```json
{
  "peers": [
    {
      "id": "macbook",
      "kind": "ssh",
      "host": "eric@192.168.1.42",
      "port": 22,
      "identity_file": "~/.ssh/id_ed25519",
      "remote_snap_dir": "~/.bettercursor/peers/bettercursor-main",
      "remote_hostname": "macbook-pro-m1"
    }
  ]
}
```

然后在 devtools console 里:

```js
await __TAURI__.invoke('transport_list_peers')          // → [{id:"macbook",...}]
await __TAURI__.invoke('transport_test', { peerId: 'macbook' })  // → {ok:true, latency_ms:42}
await __TAURI__.invoke('transport_push', { uuid: '<某 session>', peerId: 'macbook' })
// ~/.bettercursor/peers/bettercursor-main/<host>/<uuid>.json 现在写到对端了
await __TAURI__.invoke('transport_pull', { peerId: 'macbook', sinceMs: 0 })
// → { peer_id: "macbook", count: 1, snapshots: [...] }
```

SSH 安全 flag 内置: `BatchMode=yes` (不走交互式 prompt) + `StrictHostKeyChecking=accept-new` (新 host 自动加进 known_hosts, 已存在但 key 变了硬报错). v0.2.6 的 `Transport` trait 是**同步**的 (不是 `async_trait`), v0.3.0 上 outbox 时再迁 async. UI (`<SyncPeersDialog>`) 留 v0.3.0.

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

### 5. pnpm 9.4+ 拒空 `pnpm-workspace.yaml`

如果仓库根有 `pnpm-workspace.yaml` (哪怕只有 `allowBuilds`), 必须声明 `packages` 字段. 缺了 pnpm 会 `ERROR packages field missing or empty` 罢工. 至少加:

```yaml
packages:
  - "."
```

这条踩过 v0.2.5 第一次 release 的坑 — 所有 matrix job 30 秒全挂.

## 文档

| 文件 | 内容 |
|---|---|
| [PRD.md](PRD.md) | 产品需求 v0.1 功能矩阵 + 验收标准 |
| [SYNC_DESIGN.md](SYNC_DESIGN.md) | v0.2+ 同步功能设计文档 |
| [SYNC_DESIGN.md](SYNC_DESIGN.md) | v0.2+ 同步与跨设备设计 |
| [docs/README.md](docs/README.md) | 文档布局; 本地归档见 `docs/local/` (gitignore) |

## 路线图

```
v0.2.5 (✅ done)  跨平台打包 · i18n · 后台 sync · 对话记录 · 修复 orphan · 删除
v0.2.6 (✅ now)   跨设备 sync — Transport trait 初版 · SSH/rsync (T2) impl
                  · 4 个 Tauri 命令 · Intel dmg
v0.3.0 (✅ done)   ~/.bettercursor/unified.db · snapshot codec v4 ·
                  async Transport · Conflict 5-way
v0.3.1 (✅ done)   LAN mDNS 配对 · outbox · sync loop
v0.3.2 (✅ now)   <SettingsDialog> UI 整合 · i18n 修复 · 侧栏体验
v0.3.3+           T3 Git · T4 S3 · T5 Tailscale adapter
```

## 致谢

- UI 范式来自 [farion1231/cc-switch](https://github.com/farion1231/cc-switch)
- 旧版 Python 守护进程 (`bettercursor/`, `adapter/`) 提供了解析算法参考
- `vendored/cursaves/` (AGPL, 只读) 与 `vendored/cursor-history/` (MIT, 只读) 是上游 Cursor 解析库快照; 可借鉴算法索引见 [SYNC_DESIGN.md §11.5](SYNC_DESIGN.md)

---

> 当前为个人早期项目. v0.2.6 是首个带跨设备 sync 能力的 release.
