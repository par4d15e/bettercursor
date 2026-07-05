# bettercursor — PRD (Product Requirements Document)

> 文档目的: 一个 **Tauri + Rust 桌面应用**, 参照 [cc-switch-session.png](cc-switch-session.png) 的"会话管理"界面, 列出本机 Cursor 产生的所有 session 并标注来源 (Mac / Linux CLI / Linux Desktop).
>
> 配套文档:
>   - [BACKGROUND.md](BACKGROUND.md) — 调研 + 发现的考古记录
>   - [goal.md](goal.md) — 用户原始需求 (3 条)
>   - [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) — Tauri+Rust 实施计划
>   - [SYNC_DESIGN.md](SYNC_DESIGN.md) — **后续能力 (v0.2+) 设计稿: 本地同步 / 跨设备 / snapshot codec**
>
> **状态**:
>   - **v0.1** (2026-07-03 完工) = **只读 session 查看器**, 1:1 还原 cc-switch 界面. 详见 §0.
>   - **v0.2-alpha** (2026-07-03 完工) = **手动单 session L2↔L3 补层 sync**, 单按钮一键补齐缺失层. 详见 [SYNC_DESIGN §0.5](SYNC_DESIGN.md).
>   - **v0.2.1** (2026-07-04 完工) = 修 orphan (latestRootBlobId 空字符串) + 删除 session (L1 + L2, L3 跳). 详见 [SYNC_DESIGN.md §9](SYNC_DESIGN.md).
>   - **v0.2.2** (2026-07-04 完工) = 对话记录展开 — L1+L2+L3 三路合并 (bubble-id 对账 + 字段级 LWW) + `<MessageList>` 薄包装 (sticky header + 浮动跳转 + stable key). 详见 [SYNC_DESIGN.md §7](SYNC_DESIGN.md).
>   - **v0.2.3** (2026-07-04 完工) = 后台 sync loop 收尾 — `refresh_sessions` 改名为 `sync_now` + `watcher_status.last_scan_at_ms` 暴露给前端 + `<SyncNowButton>` (立即扫描) + `<SyncStatusBadge>` ("● 自动同步 · Xs 前"). 详见 [SYNC_DESIGN.md §10.1](SYNC_DESIGN.md).
>   - **v0.2.5** (2026-07-04 完工) = 跨平台打包 + i18n — version bump 三件套 (0.1.0→0.2.5) + `bundle.macOS` (未签名 dmg, Mac 10.15+) + `bundle.linux.deb` depends + react-i18next (zh-CN/en) + `<LanguageSwitcher>` (localStorage 持久化) + GitHub Actions release workflow (ubuntu+macos+windows matrix). 详见 [README.md](README.md) §下载安装.
>   - **v0.2.6** (2026-07-04 完工) = cross-device sync — **Transport trait 初版 + SSH/rsync (T2) impl + 4 个 Tauri 命令** (`transport_list_peers` / `transport_test` / `transport_push` / `transport_pull`) + `~/.bettercursor/transports.json` peer 配置 + 同步 trait (有意识偏离 SYNC_DESIGN §4.4 spec 的 async_trait; v0.3.0 上 outbox 时再迁). 同步 metadata-only snapshot (8 字段, 不含 bubbles/blobs — 那是 v0.3.0 unified.db + 完整 codec 的活). **无 UI** (出 SyncPeersDialog 留 v0.3.0); 用法靠 `invoke('transport_*')` + 手动编辑 transports.json. 详见 [SYNC_DESIGN.md §4.4 / §10.1 / §11](SYNC_DESIGN.md).
>   - **v0.2.6 housekeeping** (2026-07-04 完工, 跟 v0.2.6 一起打包发布) = CI matrix 加 `macos-13` (Intel x64 dmg) + Node 20→22 + vitest 2 + jsdom 25 + `@testing-library/react` 16 + `<SyncStatusBadge>` / `<BrokenBadge>` i18n-aware 单测 (15 case). 无业务代码改动.
>   - **v0.3.0+** = unified.db (§3) + 完整 snapshot codec v4 (bubbles / blob_refs / raw_blobs) + 5-way Conflict enum + 离线 outbox + 异步 Transport trait + `<SyncPeersDialog>` UI + git / S3 / Tailscale / folder watcher 多种 transport.

---
## 0. v0.1 Status (2026-07-03 完工)

**v0.1 = 只读 session 查看器**. 1:1 还原 cc-switch 的"会话管理"界面, 全程本地, 不走云.

### 0.1 已实现

| 能力 | 实现位置 | 验证 |
|------|---------|------|
| Tauri v2 + React 19 + TS + Vite + Tailwind 项目骨架 | `src-tauri/`, `src/`, `package.json` | `pnpm build` ✓ / `cargo check` ✓ |
| 4 层路径解析 (跨平台 Mac / Linux / Windows) | `src-tauri/src/core/paths.rs` | `cargo test chat_root_matches_python` ✓ (MD5 跟 Python 一致) |
| WAL-safe SQLite 读 (cursor-agent 的 store.db + Electron 的 state.vscdb) | `src-tauri/src/core/storage.rs` | `cargo test read_missing_db_errors` ✓ |
| 3 层扫描 + 按 uuid 合并 (canonical) | `src-tauri/src/core/canonical.rs` | 单元 + 集成 |
| 4 个 Tauri command: `list_sessions` / `refresh_sessions` / `get_resume_command` / `platform_info` | `src-tauri/src/lib.rs` | Tauri invoke 通路 |
| React UI: 左树 + 右详情 + 来源标签 (蓝/绿/紫) + 搜索 + 刷新 + 复制 resume | `src/components/{SessionTree,SessionDetail,SourceBadge}.tsx` | `pnpm build` 1588 modules ✓ |
| Zustand 状态管理 + Tauri event 监听 | `src/store/sessionStore.ts` | `pnpm exec tsc --noEmit` ✓ |

### 0.2 工作原理

```
[Tauri 启动]
   ↓
[setup() 钩子: 异步扫 3 层, 写 AppState, emit 'sessions-updated']
   ↓
[React useEffect: listSessions() → 渲染 SessionTree]
   ↓
[用户操作]  →  [refresh] 触发 listSessions 重拉
            →  [select session] 渲染 SessionDetail
            →  [copy resume] 调 get_resume_command + writeText
```

### 0.3 数据流

```
Layer 1 (JSONL) ─┐
Layer 2 (store.db)├─→ canonical::scan_all() ─→ Vec<CanonicalSession>
Layer 3 (state.vscdb)─┘                            ↓
                                          Tauri::State<AppState>
                                                  ↓
                                            Tauri command
                                                  ↓
                                            React (invoke)
                                                  ↓
                                          SessionTree 渲染
```

### 0.4 关键文件清单

| 文件 | 行数 | 作用 |
|------|-----|------|
| `src-tauri/src/lib.rs` | 113 | Tauri 入口 + 4 个 command + setup() |
| `src-tauri/src/core/paths.rs` | 138 | 4 层路径 + MD5 chat_root (parity test) |
| `src-tauri/src/core/storage.rs` | 200 | WAL-safe SQLite 读 + ItemTable / cursorDiskKV / blobs / meta 4 张表 |
| `src-tauri/src/core/canonical.rs` | 330 | 扫 3 层 + 合并 + SourceInfo / Sources / CanonicalSession |
| `src/App.tsx` | 14 | 顶层 split layout |
| `src/components/SessionTree.tsx` | 175 | 左面板 (cc-switch 风格) |
| `src/components/SessionDetail.tsx` | 145 | 右面板 + 复制 resume |
| `src/components/SourceBadge.tsx` | 22 | 三色徽章 |
| `src/store/sessionStore.ts` | 100 | Zustand + 过滤 + 分组 |

### 0.5 没实现 (v0.2+)

| 能力 | 在哪设计 | 状态 |
|------|---------|------|
| 写 store.db / state.vscdb (delete 或 sync 都需要) | [SYNC_DESIGN.md §3](SYNC_DESIGN.md) | **v0.2-alpha ✅** (单 session L2/L3 补层) |
| 修 `latestRootBlobId` (orphan session 修复) | [SYNC_DESIGN.md §3.3](SYNC_DESIGN.md) | **v0.2-alpha ✅** (inline `fix_latest_root` in `core::sync`) + Python 参考已实现 (`bettercursor/blob_dag.py`) |
| 手动 L2↔L3 补层 sync (`sync_session_layer23` command) | [SYNC_DESIGN.md §4.3](SYNC_DESIGN.md) | **v0.2-alpha ✅** (UI 单按钮) |
| Cursor 进程检测 (sync 安全锁) | `src-tauri/src/core/process.rs` | **v0.2-alpha ✅** |
| **v0.2.1 — Tauri command `fix_orphans` (扫所有 store.db, 自动修 root)** | [SYNC_DESIGN.md §4.1](SYNC_DESIGN.md) | **v0.2.1 ✅** |
| **v0.2.1 — Tauri command `delete_session` (Layer 1 + Layer 2 直 rm, L3 跳)** | [SYNC_DESIGN.md §4.1](SYNC_DESIGN.md) | **v0.2.1 ✅** |
| **v0.2.1 — UI: SessionTree 头部 "Wrench" 按钮 + toast** | [SYNC_DESIGN.md §9](SYNC_DESIGN.md) | **v0.2.1 ✅** |
| **v0.2.1 — UI: SessionDetail 单条 "修复 Layer 2" + 启用 "删除" + 原生 `<dialog>` 确认** | [SYNC_DESIGN.md §9](SYNC_DESIGN.md) | **v0.2.1 ✅** |
| **v0.2.2 — 对话记录展开 (L1+L2+L3 三路合并 + bubble-id 对账 + 字段级 LWW)** | [SYNC_DESIGN.md §7](SYNC_DESIGN.md) | **v0.2.2 ✅** |
| **v0.2.2 — `<MessageList>` 薄包装 (sticky header + 三态文案 + 浮动跳转底部 + stable key)** | [SYNC_DESIGN.md §9](SYNC_DESIGN.md) | **v0.2.2 ✅** |
| **v0.2.2 — `Bubble` 加 `id` / `created_at_ms` (均 `#[serde(default)]`)** | `src-tauri/src/core/canonical.rs` | **v0.2.2 ✅** |
| **v0.2.2 — `paths::find_layer1_jsonl_for` 合并两份 finder + `inject::LayerBubble` 改为 `canonical::Bubble` 别名** | `src-tauri/src/core/paths.rs` + `src-tauri/src/core/inject.rs` | **v0.2.2 ✅** |
| 后台 sync loop (notify + 30s polling fallback + 500ms debounce) | [SYNC_DESIGN.md §5](SYNC_DESIGN.md) | **v0.2-alpha ✅** (永跑, 无 toggle, 见 #103) |
| `sync_now` command (用户手动触发全量扫描) | [SYNC_DESIGN.md §4](SYNC_DESIGN.md) | **v0.2.3 ✅** (从 v0.1 `refresh_sessions` 改名) |
| `get_sync_status` (`watcher_status` 加 `last_scan_at_ms`) | [SYNC_DESIGN.md §4](SYNC_DESIGN.md) | **v0.2.3 ✅** (frontend `<SyncStatusBadge>` 显示 "● 自动同步 · Xs 前") |
| 不复活 `auto_sync_enabled` toggle (沿用 #103 拍板) | — | **v0.2.3 ✅** (默认行为不变, 只暴露状态) |
| **v0.2.5 — 三件套 version bump (0.1.0 → 0.2.5)** | `package.json` + `src-tauri/Cargo.toml` + `src-tauri/tauri.conf.json` | **v0.2.5 ✅** |
| **v0.2.5 — `bundle.macOS` 子配置 (最低系统 10.15, 未签名 `signingIdentity: null`, dmg 窗口 660×400)** | `src-tauri/tauri.conf.json` | **v0.2.5 ✅** |
| **v0.2.5 — `bundle.linux.deb.depends` (libwebkit2gtk-4.1-0 / libgtk-3-0 / libayatana-appindicator3-1)** | `src-tauri/tauri.conf.json` | **v0.2.5 ✅** |
| **v0.2.5 — react-i18next 接入 + zh-CN/en 两套资源 (110 条 UI 字符串)** | `src/i18n/index.ts` + `src/locales/{zh-CN,en}.json` | **v0.2.5 ✅** |
| **v0.2.5 — `<LanguageSwitcher>` (header `<select>`, localStorage 持久化, `i18n.changeLanguage()` 即时切换)** | `src/components/LanguageSwitcher.tsx` | **v0.2.5 ✅** |
| **v0.2.5 — GitHub Actions release workflow (ubuntu+macos+windows matrix, tag `v*.*.*` 触发, softprops/action-gh-release@v2 自动发版)** | `.github/workflows/release.yml` | **v0.2.5 ✅** |
| **v0.2.6 — `Transport` trait (4 方法: push / pull / list_remote / endpoint_id, 同步签名 — 有意偏离 SYNC_DESIGN §4.4 spec 的 async_trait)** | `src-tauri/src/core/transport/mod.rs` | **v0.2.6 ✅** |
| **v0.2.6 — `SshRsyncTransport` (T2) impl: 调系统 `ssh` / `rsync`, 0 新 Cargo dep, `BatchMode=yes` + `StrictHostKeyChecking=accept-new` 安全 flag, heredoc 写 + atomic rename** | `src-tauri/src/core/transport/ssh.rs` | **v0.2.6 ✅** |
| **v0.2.6 — `SessionSnapshot` 最小载体 (8 字段: uuid / 时间戳 / host / project_slug / project_path / source_path / text_preview 280 字符 / bubble_count; metadata-only, 不含 bubbles/blobs)** | `src-tauri/src/core/transport/snapshot.rs` | **v0.2.6 ✅** |
| **v0.2.6 — `TransportConfigFile` + `PeerConfig` (id / kind / host / port / identity_file / remote_snap_dir / remote_hostname) + 原子 save (`*.tmp` + rename)** | `src-tauri/src/core/transport/config.rs` | **v0.2.6 ✅** |
| **v0.2.6 — 4 个 Tauri 命令: `transport_list_peers` / `transport_test` / `transport_push` / `transport_pull` (同步, 走 `State<'_, AppState>` 拿 session cache)** | `src-tauri/src/lib.rs` | **v0.2.6 ✅** |
| **v0.2.6 — 前端 4 个 IPC wrapper + 4 个 type interface (`PeerSummary` / `TestReport` / `PushReport` / `PullReport` / `RemoteSession`)** | `src/lib/tauri.ts` | **v0.2.6 ✅** |
| **v0.2.6 — `~/.bettercursor/transports.json` peer 配置文件 (新文件, 跟 `~/.bettercursor/config.json` 分开)** | `core::transport::config::TransportConfigFile` | **v0.2.6 ✅** |
| **v0.2.6 — Rust 单元测试 20 case (snapshot codec round-trip / source_path 优先级 / text_preview 280 截断 / config serde round-trip / ssh_cmd 安全 flag / push ssh failure stderr / endpoint_id / with_bins)** | `src-tauri/src/core/transport/{mod,snapshot,ssh,config}.rs::tests` | **v0.2.6 ✅** |
| **v0.2.6 — `tests/fixtures/fake-ssh.sh` + `fake-rsync.sh` mock 二进制 (写 argv log + 可模拟 fail)** | `src-tauri/tests/fixtures/` | **v0.2.6 ✅** |
| **v0.2.6 housekeeping — CI matrix 加 `macos-13` (Intel x64 dmg 跟 Apple Silicon dmg 一起出)** | `.github/workflows/release.yml` | **v0.2.6 ❌ superseded** — v0.2.6 release fix 又删掉了 (见下) |
| **v0.2.6 release fix — macOS 支持策略改为 arm64 only: 删 CI matrix 里 `macos-13` 一行 (GitHub Actions `macos-13` runner pool 容量满, 持续卡 release); 加 dmg rename step (`_aarch64.dmg` → `_arm64.dmg`) 走 Apple marketing 命名** | `.github/workflows/release.yml` | **v0.2.6 ✅** |
| **v0.2.6 release fix — `macOS` 支持策略: Apple Silicon only (Intel Mac 退出支持). 文档依据: 2026-07-04 项目拍板 + Apple 2020 起停售 Intel Mac + `macos-13` runner pool 不可靠** | `PRD.md §0.5` + `BACKGROUND.md §14` | **v0.2.6 ✅** |
| **v0.2.6 housekeeping — Node 20 → 22 (CI)** | `.github/workflows/release.yml` | **v0.2.6 ✅** |
| **v0.2.6 housekeeping — vitest 2 + jsdom 25 + `@testing-library/react` 16 + 15 case 测 `<SyncStatusBadge>` / `<BrokenBadge>` i18n-aware fallback** | `vitest.config.ts` + `src/test/setup.ts` + `src/components/*.test.tsx` | **v0.2.6 ✅** |
| **v0.3.0 PR-1 — `~/.bettercursor/unified.db` (§3)**: 7+1 表 (`schema_version` / `sessions` / `bubbles` / `bubbles_fts` / `blobs` / `composer_data` / `sync_runs` / `archive` / `conflicts`) + FTS5 虚表 (`unicode61 remove_diacritics 2` tokenizer, 无 triggers 手动维护) + `UnifiedDb::rebuild_from_cursor_state` 幂等 ingest + `record_archive` / `record_conflict` / `record_sync_run` / `finish_sync_run` / `search_bubbles` / `delete_session_row` / `unresolved_conflicts` helpers. PRAGMA `journal_mode=WAL` + `foreign_keys=ON` + `synchronous=NORMAL`. 0 新 Cargo dep (rusqlite+bundled+sha2+hex+chrono+serde+tempfile 全部已 in). | `src-tauri/src/core/unified.rs` (~600 行) + `paths::unified_db_path()` | **v0.3.0 PR-1 ✅** |
| **v0.3.0 PR-1 — `Bubble.parent_bubble_id: Option<String>` 字段 (v0.3.0 first cut 全部 None, v0.3.1+ 启发式回填)** | `core/canonical.rs::Bubble` | **v0.3.0 PR-1 ✅** |
| **v0.3.0 PR-1 — `ComposerData { full_json, subset_json }` + `CanonicalSession.{composer_data, composer_id}` + `Sources::preferred_endpoint_kind()` + `Sources::preferred_source_path()` (mac > linux_desktop > linux_cli 优先级). `scan_layer3_into` per-composer loop 末尾捕获 `composerData` 全文, 避免 unified.db write 时回 L3 重读** | `core/canonical.rs` | **v0.3.0 PR-1 ✅** |
| **v0.3.0 PR-1 — Migration A coexist: v0.2.6 inline-write 路径 (`write_layer2` / `write_layer3` / `fix_latest_root` / `delete_session` L1+L2) 保留; 4 个 hook 点 (`sync_session` / `fix_orphans` / `delete_session` / `sync_now`) 同步写 unified.db. unified.db 是 read-cache + archive + sync_runs, 真实写仍走 L1+L2** | `core/sync.rs` (3 处 hook) + `lib.rs::sync_now` (1 处 hook) | **v0.3.0 PR-1 ✅** |
| **v0.3.0 PR-1 — 单元测试 10 case (`open_creates_eight_tables` / `rebuild_is_idempotent` / `rebuild_writes_content_hash_deterministically` / `archive_and_delete_cascade` / `resolve_conflict_marks_resolved` / `sync_run_record_and_finish` / `rebuild_honors_sources_priority_order` / `content_hash_changes_when_text_changes` / `sources_preferred_helpers_four_cases` / `bubble_helper_round_trip`)** | `core/unified.rs::tests` | **v0.3.0 PR-1 ✅** |
| **v0.3.0 pre-PR-2 — L3 bubble 完整文本提取** (`toolFormerData` / thinking / codeBlocks + toolCalls + timestamp fallback) | `core/canonical.rs::extract_l3_bubble_text` | **v0.3.0 pre-PR-2 ✅** |
| **v0.3.0 pre-PR-2 — Cursor 3.0+ session discovery** (`composer.composerHeaders` / workspace `selectedComposerIds` / `composerChatViewPane.*` / bubble count 过滤) | `core/canonical.rs::scan_layer3_into` | **v0.3.0 pre-PR-2 ✅** |
| **v0.3.0 pre-PR-2 — timestamp gaps + parity fixtures** (`fill_timestamp_gaps` + `tests/fixtures/cursor-history/`) | `core/canonical.rs` | **v0.3.0 pre-PR-2 ✅** |
| **v0.3.0 PR-2 — snapshot codec v4** (`SNAPSHOT_VERSION=4`, `from_canonical_v4` / encode / decode / `write_snapshot_file` atomic) | `core/snapshot.rs` | **v0.3.0 PR-2 ✅** |
| **v0.3.0 PR-2 — `core::conflict.rs` 五态分类** (`classify` / `bubble_diff` / `auto_merge` / `content_hash_from_bubbles`) | `core/conflict.rs` | **v0.3.0 PR-2 ✅** |
| **v0.3.0 PR-2 — Transport async 化** (`tokio` + `async-trait`, `ssh.rs` tokio::process, Tauri 命令内部 `block_on`) | `core/transport/{mod,ssh}.rs` | **v0.3.0 PR-2 ✅** |
| **v0.3.0 PR-2 — `transport_pull` → v4 decode + 5-way classify + `unified.upsert_session_from_snapshot`** | `lib.rs` + `unified.rs` | **v0.3.0 PR-2 ✅** |
| **v0.3.0 PR-2 — `snapshot_meta.rs` 重命名** (v0.2.6 8-field push 载体保留) | `core/transport/snapshot_meta.rs` | **v0.3.0 PR-2 ✅** |
| **v0.3.0 PR-2 — agentKv 写入 (最小切片)** (`write_layer3` 从 `conversationState` 提取 blob id 并复制 `agentKv:blob:{hex}`) | `core/sync.rs` | **v0.3.0 PR-2 ✅** |
| **v0.3.0 PR-2 — 单元测试 28+ 新 case** (snapshot 5 / conflict 8 / ssh async 4 / sync agentKv 1; 全量 `cargo test --lib` 126 case) | 各 `::tests` | **v0.3.0 PR-2 ✅** |
| 跨设备 (Mac↔Linux) | [SYNC_DESIGN.md §4/§5](SYNC_DESIGN.md) | **v0.3.0 后端 ✅** (unified.db + codec v4 + 5-way conflict + async Transport); **v0.3.1 开箱即用 ✅** — T2a LAN mDNS + 配对 + `SyncPeersDialog` / `ConflictResolveDialog`; SSH (T2b) 保留为高级模式 |
| 对话记录展开 (读 store.db blobs + JSONL messages) | [SYNC_DESIGN.md §7](SYNC_DESIGN.md) | **v0.2.2 ✅** |
| **L3 bubble 完整文本提取** (`toolFormerData` / thinking / codeBlocks, 非仅 `text` 字段) | [SYNC_DESIGN.md §2.8](SYNC_DESIGN.md) | **v0.3.0 pre-PR-2 ✅** |
| **Cursor 3.0+ session discovery** (`composer.composerHeaders` / `selectedComposerIds` / `composerChatViewPane.*` / workspace DB 补全) | [SYNC_DESIGN.md §11.5](SYNC_DESIGN.md) | **v0.3.0 pre-PR-2 ✅** |
| **agentKv blob 写入 + 缺失修复** (无则 Desktop `--resume` 报 Blob not found) | [SYNC_DESIGN.md §9.8](SYNC_DESIGN.md) | **v0.3.0 PR-2 ✅** (最小切片: `write_layer3` 复制已有 agentKv) |
| **L3 统一写 API** (batch + backup 保留 N 份 + verify) | [SYNC_DESIGN.md §9.8](SYNC_DESIGN.md) | ⚪ 部分 — `sync.rs` 有 inline write; 缺 cursaves 级 backup 策略 → PR-2b |
| **`core::conflict.rs` 五态分类** | [SYNC_DESIGN.md §6](SYNC_DESIGN.md) | **v0.3.0 PR-2 ✅** |
| **Doctor 孤儿会话审计/恢复** (L3 有 composerData 但未注册 sidebar) | [SYNC_DESIGN.md §11.5](SYNC_DESIGN.md) | ⚪ PR-2b |
| **parity fixtures** (cursor-history spec 010–013 场景 → Rust 单测) | [SYNC_DESIGN.md §11.5](SYNC_DESIGN.md) | **v0.3.0 pre-PR-2 ✅** |
| UI: SyncPeersDialog / 推送按钮 / sync history | [SYNC_DESIGN.md §9](SYNC_DESIGN.md) | 设计稿 (v0.3.1) |
| Mac 端 cross-compile / dmg 打包 | Phase T4 (PRD §7) | **v0.2.5 ✅** (Apple Silicon) + **v0.2.6 ✅** (Intel x64 via `macos-13` matrix) |

### 0.6 怎么跑

```bash
cd /home/eric/workspace/bettercursor
pnpm tauri dev          # 开发模式, 第一次编译 5-10 分钟
# 或
pnpm tauri build        # 打包 (deb / AppImage / dmg, 平台相关)
```

### 0.7 测试覆盖

```bash
# Rust 单元测试 (3 个)
cd src-tauri && cargo test --lib
  ✓ core::paths::tests::chat_root_matches_python
  ✓ core::paths::tests::sanitize_strips_slashes
  ✓ core::storage::tests::read_missing_db_errors

# 前端 typecheck + build
pnpm exec tsc --noEmit     # TypeScript ✓
pnpm build                  # Vite 1588 modules, 208 KB JS / 10 KB CSS

# Python 参考 (回归基线, 验证 Rust 端口与原 Python 等价)
cd .. && python3 tests/test_layer2_import.py
  ✓ Layer 2 import OK
  ✓ 80 blobs written to store.db
  ✓ root auto-fixed
```

---

## 1. 背景与一句话定位

**问题**: Cursor IDE 的 session 数据散落在 4 个独立的 SQLite/JSONL 存储层. 用户在多端工作 (Mac Electron / Linux CLI / Linux Electron Desktop) 时, **没有统一的"我有哪些 session / 哪些还在跑 / 哪些是空的"视图**. Cursor 原生 Sidebar 只能看本端, 切到另一端全无.

**产品**: `bettercursor` — **Tauri + Rust 只读 session 查看器**, 1:1 参照 cc-switch 的"会话管理"界面:
- **左面板**: 树形列表, 根节点 "Cursor" → 子目录 = project_slug → 叶子 = session
- **右面板**: 选中 session 后, 显示标题 / 元信息 / 来源 / 复制 resume 命令 / 对话记录
- **来源标签**: 每条 session 行右侧小灰底圆角徽章 (Mac Desktop / Linux CLI / Linux Desktop)

**非目标 (用户明确)**:
- ❌ 不做自动同步 (goal.md #2)
- ❌ 不做手动单次同步 (goal.md #3)
- ❌ 不做 daemon / SSH / 跨设备
- ❌ 不写 SQLite / JSONL (除未来可选的 delete)
- ❌ 不监听文件变更 (用户点 refresh 按钮才重扫)

**定位** (与 cc-switch 对比):
- **cc-switch** 的"会话管理"屏: 列 Codex session, 树形, 详情可删 → 我们列 Cursor session, 同样树形 + 详情, 但只读 (至少 v0.1)
- **bettercursor** 不是 cc-switch 替代品, 只是为 Cursor 提供同款 UI 的本地工具

---

## 2. 用户故事与验收标准

### 2.1 目标用户

**Eric (你)** — Cursor Pro 用户. 日常在 Mac (Electron) / Linux (CLI) / Linux (Electron Desktop) 多端切换. 想"一眼看完全部 session 在哪、什么时候更新过、能不能 resume" — 这个视图 Cursor 原生不提供.

### 2.2 用户故事 (最小集)

| ID | 故事 | 验收 |
|----|------|------|
| US-1 | 我打开 bettercursor, **看到本机所有 Cursor session**. | 树形列表, 根 "Cursor", 子目录 = project_slug, 叶子 = session. 至少 17 条 (与 sessions.csv 一致). |
| US-2 | 我看到每条 session **带来源标签** (Mac / Linux CLI / Linux Desktop). | `<SourceBadge>` 三色, 视觉上一眼区分. |
| US-3 | 我点一条 session, **看到详情** (标题 / 时间 / 项目 / uuid / 对话记录). | 右面板渲染, cc-switch 同款布局. |
| US-4 | 我点 "复制 resume 命令" 按钮, 拿到 `cursor-agent --resume <uuid>`. | 剪贴板里有正确命令. |
| US-5 | 我用搜索框过滤 session 名称 / 项目 / 内容关键字. | 实时过滤, 高亮匹配. |
| US-6 | 我点 "刷新" 按钮, **重新扫描** 本机存储. | 1-2 秒内列表更新. |
| US-7 | 重复的 UUID (同一 session 在多层都存在) **不出现两次**. | canonical merge 按 uuid + project_slug dedup. |

### 2.3 非目标 (Non-goals, 强调)

- ❌ **不写 SQLite / JSONL** (除可选 delete, 不在 v0.1)
- ❌ **不同步任何端** (用户 2026-07-03 明确去掉)
- ❌ **不监听文件变更** (用户点 refresh 才重扫)
- ❌ **不做 daemon** (无后台进程)
- ❌ **不跨设备** (Linux 上跑就是 Linux 视图; Mac 上跑就是 Mac 视图; 不做 Tailscale mesh 跨设备)
- ❌ **不修改 Cursor 内部代码** (不打补丁不装插件)
- ❌ **不抓取对话内容做搜索/向量化** (只读首条 user message 做 preview)


## 3. 架构总览

### 3.1 单端架构 (本机运行)

```
┌─────────────────────────────────────────────────────────────────────┐
│  本机: Mac 或 Linux (Tauri 桌面应用)                                 │
│                                                                     │
│  ┌────────────────────┐         ┌─────────────────────────────┐   │
│  │  React Frontend    │ ←invoke→│  Rust Backend (Tauri cmd)   │   │
│  │  (WebView)         │         │  ───────────────────────    │   │
│  │                    │         │  • paths::scan()            │   │
│  │  • SessionTree     │         │  • storage::read_*()        │   │
│  │  • SessionDetail   │         │  • canonical::merge()       │   │
│  │  • SourceBadge     │         │                             │   │
│  │  • SearchBar       │         │  → Vec<CanonicalSession>    │   │
│  │  • RefreshButton   │         │                             │   │
│  └────────────────────┘         └─────────────────────────────┘   │
│                                          │                          │
│                                          ↓ 只读                      │
│              ┌────────────────┬──────────────────┬──────────────┐  │
│              │ Layer 1 (JSONL)│ Layer 2 (store.db)│ Layer 3      │  │
│              │ ~/.cursor/     │ ~/.cursor/chats/  │ ~/.config/   │  │
│              │   projects/    │   <md5>/<uuid>/   │   Cursor/    │  │
│              │   <slug>/      │                   │   User/      │  │
│              │   agent-       │   chats/<md5>/    │   global-    │  │
│              │   transcripts/ │   <uuid>/         │   Storage/   │  │
│              │   <uuid>/      │   store.db        │   state.vscdb│  │
│              │   *.jsonl      │                   │              │  │
│              └────────────────┴──────────────────┴──────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.2 启动流 (Tauri app)

```
1. Tauri 启动
2. main() → bettercursor_lib::run()
3. setup() 钩子:
   a. canonical::scan_all_sessions() 扫 3 个存储层 (Layer 1/2/3)
   b. canonical::merge() 去重 + 合并 sources
   c. 存到 AppState.sessions
   d. emit 'sessions-ready' 给前端
4. 前端 useEffect(() => invoke('list_sessions')) 拉数据
5. 渲染 <SessionTree> + <SourceBadge> + <SessionDetail>
```

### 3.3 不在范围内 (强调)

- ❌ 无 daemon
- ❌ 无 SSH 服务
- ❌ 无 file watcher
- ❌ 无后台任务
- ❌ 无跨设备同步

每次 Tauri 启动 = 一次扫描. 用户点 refresh = 再扫一次. 关闭 = 内存释放.

---

## 4. 数据模型

### 4.1 Canonical session record (跨端统一格式)

```json
{
  "uuid": "c1ea7999-005a-434f-bcf4-da8ddd9ff066",
  "project_slug": "home-eric-workspace-enenzuo",
  "project_path": "/home/eric/workspace/enenzuo",
  "chat_root": "c19d07070edc77b1fdcdaf0dfecaf97f",
  "workspace_identifier": "946eda0d4e927e1d340b92790f030093",
  "name": "WeChat profile 设计",
  "last_updated_at": 1783052073432,
  "bubble_count": 11,
  "is_empty_draft": false,
  "sources": {
    "mac":          {"last_seen_at": 1783052073432, "layer": "3'"},
    "linux_cli":    {"last_seen_at": null,           "layer": null},
    "linux_desktop":{"last_seen_at": null,           "layer": null}
  },
  "first_user_message_preview": "读完 README.md...",
  "files_referenced": ["README.md", "src/main.ts"]
}
```

### 4.2 4 层存储的写入规则

| 层 | 路径 | 写入 daemon 行为 |
|---|------|-----------------|
| Layer 1 (JSONL) | `~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl` | **只读**, 不写. 这是 Cursor 自己的工作区同步, daemon 不能篡改. |
| Layer 2 (CLI store.db) | `~/.cursor/chats/<md5>/<uuid>/store.db` | **写**: 把 Mac/Linux Desktop 来的 agentBlobs + composerData 写入. **修**: 调 `fix_orphan_sessions.py` 保证 `meta[0].latestRootBlobId` 有效. |
| Layer 3 (Linux Desktop) | `~/.config/Cursor/User/globalStorage/state.vscdb` | **写**: ItemTable.composer.composerHeaders + cursorDiskKV.composerData:<uuid> + cursorDiskKV.bubbleId:<uuid>:<bid>. 写前必须关 Desktop 或用 WAL-safe 模式. |
| Layer 3' (Mac) | `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` | Mac client 自己写, 不通过 Linux daemon. |

### 4.3 冲突解决

| 场景 | 策略 |
|------|------|
| 同一 UUID, daemon 收到 Mac + Linux Desktop 两条推送 | 比较 `last_updated_at`, 新的覆盖旧的; 旧的备份到 `~/.bettercursor/archive/<uuid>/<timestamp>.json` |
| 同一 UUID 在两边同时有不同 bubble ID | merge set (并集), 重复的取 content hash 相同的那个 |
| Mac 和 Linux Desktop 都想给同一 UUID 设 workspaceIdentifier | 取有值的那一个; 都没有用 §6.4 的推断规则 |

---

## 5. 接口规范 (Tauri Commands)

> v0.1 范围: 只暴露**只读** command. 写操作 (delete) 在 v0.2 再说.

### 5.1 `list_sessions` (前端 → 后端)

```typescript
// 前端
const sessions = await invoke<CanonicalSession[]>('list_sessions');

// 返回
[
  {
    uuid: "c1ea7999-005a-434f-bcf4-da8ddd9ff066",
    project_slug: "enenzuo",
    project_path: "/home/eric/workspace/enenzuo",
    chat_root: "c19d07070edc77b1fdcdaf0dfecaf97f",
    name: "WeChat profile 设计",
    last_updated_at: 1783052073432,
    bubble_count: 11,
    is_empty_draft: false,
    sources: {
      linux_cli: { last_seen_at: 1783052073432, layer: "2", path: "/home/eric/.cursor/chats/.../store.db" }
    },
    first_user_message_preview: "读完 README.md...",
    files_referenced: ["README.md", "src/main.ts"]
  }
]
```

### 5.2 `get_session_detail`

```typescript
const detail = await invoke<SessionDetail>('get_session_detail', { uuid });
// 返回: { ...CanonicalSession, messages: [{role: "user"|"assistant", content, timestamp}] }
```

### 5.3 `get_resume_command`

```typescript
const cmd = await invoke<string>('get_resume_command', { uuid, source: 'linux_cli' });
// 返回: "cursor-agent --resume c1ea7999-005a-434f-bcf4-da8ddd9ff066"
```

按 source 分:
- `linux_cli` → `cursor-agent --resume <uuid>`
- `mac` / `linux_desktop` → `open -a "Cursor" --args --resume <uuid>` (注: 需 Cursor 1.0+ 支持)

### 5.4 `refresh_sessions`

```typescript
await invoke<void>('refresh_sessions');
// 后端重新扫 3 层, 更新 AppState, emit 'sessions-updated'
```

### 5.5 `delete_session` + `fix_orphans` (v0.2.1 ✅)

```typescript
// 删除 session 的 Layer 1 (JSONL) + Layer 2 (store.db). L3 强制跳过.
const report = await invoke<DeleteReport>('delete_session', {
  uuid: '...',
  cwd: '/path/to/project',         // 必须 — 算 L2 md5 bucket
  projectSlug: 'home-user-proj',  // 来自 CanonicalSession — 算 L1 path
});
// report.removed_l1 / removed_l2 / skipped_l1 / skipped_l2 / cursor_running

// 全量扫所有 chats/<md5>/<uuid>/store.db, 修空 latestRootBlobId
const orphans = await invoke<FixOrphansReport>('fix_orphans');
// orphans.fixed / skipped / scanned
```

`delete_session` 前置 `cursor_processes_running` 锁 (跟 `sync_session_layer23` 一致).
`fix_orphans` 修之前自动留 `.backup_<ts>` 兄弟文件, 写非破坏.

### 5.6 不在范围内 (强调)

- 不存在 `export` / `import` (无跨端)
- 不存在 `set_auto_sync` / `sync_now` (全量 sync loop, v0.2.3)
- 不存在 `start_daemon` / `stop_daemon` (无 daemon)
- 不存在 `Layer 3 delete` (Cursor 自己管)

---

## 6. 待决项与边界条件

### 6.1 多源同 UUID 合并

**场景**: 同一 session 在 Layer 2 (CLI store.db) 和 Layer 3 (Desktop state.vscdb) 各有一份.
**策略**: `canonical::merge` 按 uuid 去重, 每个 source 字段保留, UI 上用 `<SourceBadge>` 标多个.
**风险**: 两个 source 的 `last_updated_at` 不一致. 用最新值作为 sort key, 详情面板里两个 source 都展示.

### 6.2 chat_root 计算

**当前**: `chat_root = MD5(cwd)`. 同一项目不同 cwd 算出来不同, 视觉上是两条 session.
**范围**: 因为不做跨设备, **这不再是问题**. Mac 端跑就是 Mac cwd, Linux 端跑就是 Linux cwd, 不会混.
**未来**: 如果以后做跨端, 改用 `git remote origin URL` 作主 key.

### 6.3 JSONL 文件很大

**风险**: 启动时扫所有 JSONL (Layer 1) 可能慢.
**缓解**: 启动只读每个 JSONL 的首行 (session 元信息), 不解析全量消息. 详情面板按需加载.
**目标**: 17 session 总启动时间 < 1 秒.

### 6.4 Cursor 升级改 schema

**风险**: Cursor 改 state.vscdb / store.db 的表结构, 读失败.
**缓解**: 
- 读用 WAL-safe 模式, 失败不损坏原文件
- schema 不匹配时, 把这个 session 标为 `error`, 不影响其他 session
- UI 上显示 "Read failed: <reason>"

### 6.5 Linux 上没装 Cursor

**场景**: 用户只装了 `cursor-agent` CLI, 没装 Electron Desktop.
**行为**: Layer 3 路径不存在, canonical::scan 跳过, `sources.linux_desktop` 全为 None. UI 上不出 Linux Desktop 徽章.

### 6.6 Mac / Linux UI 差异

**已知差异**: 
- macOS: `~/Library/Application Support/Cursor/`
- Linux: `~/.config/Cursor/`
- macOS: `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
- Linux: `~/.config/Cursor/User/globalStorage/state.vscdb`
- paths::get_cursor_user_dir() 自动用 `dirs` crate 检测, 行为同 cursaves.

### 6.7 同端多次启动 bettercursor

**行为**: 多个实例可同时跑, 都是只读, 互不干扰. SQLite 读走 temp copy, 不争用.

---

## 7. 实施路线 (接 Phase 0)

### Phase 0 — ✅ 已完成 (调研 + Python 验证)

| 任务 | 产物 |
|------|------|
| 摸排 4 层存储 | BACKGROUND.md §2 |
| CPU 实测确认 Model A | BACKGROUND.md §1.3 |
| 17 session 精确分布 | sessions.csv + BACKGROUND.md §4.1 |
| **cursaves 摸排** | 验证 snapshot 格式 |
| **Python 参考实现** | `bettercursor/paths.py` (182) + `storage.py` (254) + `layer2.py` (183) + `blob_dag.py` (188) + `snapshot.py` (198) + `conflict.py` (96) + `layer3.py` (189) |
| **c1ea7999 验证** | `tests/test_layer2_import.py` 端到端 PASS |

**注**: Python 代码**不进 runtime**, 作为 Rust 端口的参考. `vendored/cursaves/` 与 `vendored/cursor-history/` 继续只读参考; 可借鉴项见 [SYNC_DESIGN.md §11.5](SYNC_DESIGN.md).

### Phase T0 — Tauri 项目骨架 (半天, #54)

| 任务 | 验收 |
|------|------|
| 验证 Rust 工具链 + Tauri CLI v2 | `cargo tauri --version` |
| 写 `src-tauri/Cargo.toml` + `tauri.conf.json` + `src/main.rs` + `src/lib.rs` | `cargo tauri dev` 跑出空窗口 |
| 写 `src/App.tsx` 显示 "会话管理" + "Cursor" 标题 | 标题显示 |
| 装 Tailwind + shadcn 风格 | 暗色主题生效 |

### Phase T1 — Rust 只读核心 (1.5 天, #55, #56, #61)

| 任务 | 验收 |
|------|------|
| 端口 `paths.py` → `core/paths.rs` | 单元测试覆盖 4 层路径 |
| 端口 `storage.py` (读函数) → `core/storage.rs` | WAL-safe 读, temp copy 模式 |
| 新写 `core/canonical.rs` (扫 + 合并) | 17 session 实测结果与 Python 一致 |
| 写 `commands::list_sessions` / `get_session_detail` | `cargo test` 全绿 |
| 实现 `get_resume_command` (按 source 分) #62 | mac → `open -a Cursor`, linux_cli → `cursor-agent --resume` |

**关键简化**: 写函数 (`write_blobs_batch`, `import_snapshot_to_layer2`) **不在 v0.1**, 因为只读. 写函数在 Phase T3 (delete) 才需要.

### Phase T2 — UI 主面板 (2 天, #59)

| 任务 | 验收 |
|------|------|
| 装 Tailwind + shadcn/ui | 暗色 + 圆角, 跟 cc-switch 风格一致 |
| `<SessionTree>` 左面板 | 根 "Cursor" + 子目录 = project_slug + 叶子 = session |
| `<SourceBadge>` 3 种颜色 (蓝/绿/紫) | 视觉一眼区分 |
| `<SessionDetail>` 右面板 | 标题 + 元信息 + resume 命令 + 复制按钮 |
| `<MessageList>` 展开对话记录 | cc-switch 同款样式 |
| 搜索框 | 输入字符实时过滤 |
| 刷新按钮 | 调 `refresh_sessions` 重扫 |
| 选中高亮 | 点行高亮, 详情同步 |

### Phase T3 — 删除 + 修复 orphan (v0.2.1 ✅)

| 任务 | 验收 |
|------|------|
| `core::sync::fix_orphans` (pub fn) | 扫所有 chats/<md5>/<uuid>/store.db, 修空 latestRootBlobId |
| `core::sync::delete_session` (pub fn) | L1 (JSONL) + L2 (store.db) `remove_dir_all`, L3 跳 |
| Tauri command `fix_orphans` | 注册到 `invoke_handler!` |
| Tauri command `delete_session` | 注册到 `invoke_handler!`, 吃 `uuid / cwd / project_slug` |
| UI: SessionTree 头部 "Wrench" 按钮 + 4s toast | `fix_orphans` 全量入口 |
| UI: SessionDetail 单条 "修复 Layer 2" 按钮 (仅 broken 时) | `fix_orphans` 单条入口 (同一后端) |
| UI: SessionDetail "删除" 启用 + 原生 `<dialog>` 确认 (L1/L2 checkbox + L3 disabled) | `delete_session` 入口, cursor_running 时按钮 disabled |

**v0.2.1 期间达成**: 用户在 v0.1 反馈里提了两个明确要求 ("希望有修复"、"希望有删除"), 在 v0.2.1 一次性落地. L3 删除策略拍板: 跳过 (Cursor Desktop 自己管).

### Phase T4 — 跨平台 (可选, v0.2)

| 任务 | 验收 |
|------|------|
| Mac 端单独 build (Tauri cross-compile) | Mac 上能扫 Layer 3' (Mac state.vscdb) |
| UI 文案本地化 (zh-CN / en) | 切语言 |
| dmg / deb / AppImage 打包 | 三平台分发 |

### 不在路线图

- ❌ 同步任何端之间 (用户去掉)
- ❌ Daemon / SSH / Tailscale mesh (用户去掉)
- ❌ 修 orphan session (我们不写就不需要修)
- ❌ file watcher / 实时刷新 (用户接受手动 refresh)

---

## 8. 风险登记表

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| Cursor 升级改 store.db schema | 中 | 高 | 读失败时标 session 为 `error`, 不影响其他 |
| Tauri v2 在 Linux 上 WebKitGTK 编译慢 | 高 | 中 | 一次 5-10 分钟, 之后增量快. CI 缓存 |
| 17+ session 大树渲染卡 | 低 | 低 | 列表虚拟化 (@tanstack/react-virtual), 子目录默认折叠 |
| rusqlite 在 macOS 上的 keychain 锁 | 低 | 低 | 用 temp copy, 不持有句柄 |
| JSONL 文件很大 (10MB+) 启动慢 | 中 | 中 | 只读首行做 preview, 不全量 parse |
| 用户没装 cursor-agent CLI | 低 | 低 | 列表照常显示, 点 "复制" 按钮拿到命令但用不了 |
| 复制命令到剪贴板失败 (Linux 无 xclip) | 低 | 低 | fallback 到弹窗显示命令, 用户手动复制 |

---

## 9. 验收清单 (Definition of Done for v0.1)

| US | 描述 | 状态 |
|----|------|------|
| US-1 | 打开 bettercursor 看到 ≥ 17 条 session (与 sessions.csv 一致) | ✅ 实现 (`canonical::scan_all` + UI 渲染) |
| US-2 | 每条 session 有 SourceBadge (蓝/绿/紫), 一眼区分 | ✅ 实现 (`SourceBadge` 组件 + `detectSource` 逻辑) |
| US-3 | 点 session 看到详情 (标题 / 时间 / 项目 / uuid / 来源) | ✅ 实现 (`SessionDetail`) — 对话记录展开 v0.2 计划 (见 [SYNC_DESIGN §7](SYNC_DESIGN.md)) |
| US-4 | 点 "复制 resume 命令" 按钮, 剪贴板有正确命令 | ✅ 实现 (`get_resume_command` + `plugin-clipboard-manager`) |
| US-5 | 搜索框过滤有效, 实时高亮 | ✅ 实现 (`useSessionStore.setSearch` + `selectFilteredSessions`) |
| US-6 | 点 "刷新" 按钮, 1-2 秒内列表更新 | ✅ 实现 (`sync_now` command, 原 v0.1 `refresh_sessions` 改名) |
| US-7 | 重复 UUID (跨层) 不出现两次 | ✅ 实现 (`canonical::scan_all` 按 uuid 合并) |

**显式不做 (v0.1 范围外, 推到 v0.2+ 设计稿 [SYNC_DESIGN.md](SYNC_DESIGN.md))**:
- [x] ~~自动同步 toggle~~ → SYNC_DESIGN §4.2
- [x] ~~单次同步 trigger~~ → SYNC_DESIGN §4.1
- [x] ~~daemon / 后台进程~~ → SYNC_DESIGN §5 (后台 tokio loop)
- [x] ~~SSH 服务 / 跨设备~~ → SYNC_DESIGN §6 (Tailscale mesh)
- [x] ~~修 orphan session `latestRootBlobId`~~ → SYNC_DESIGN §3.3
- [x] ~~对话记录展开~~ → SYNC_DESIGN §7

**编译验证** (2026-07-03):
- `pnpm build` ✓ — 1588 modules, 208 KB JS / 10 KB CSS
- `cargo check` ✓ — 0 errors, 9 warnings (unused code, 都是 v0.2 用的)
- `cargo test --lib` ✓ — 3/3 passed (含 chat_root 与 Python MD5 parity)

---

## 10. 时间线 (累加 Phase 0)

| 日期 | Phase | 进展 |
|------|-------|------|
| 2026-07-02 上午 | 调研 | 5 JSONL-only session, cursync-import.py 骨架 |
| 2026-07-02 下午 | 调研 | 确认 cursor-server 不写 SQLite |
| 2026-07-02 深夜 | 调研 | 4 层独立索引, 17 session 分布 |
| 2026-07-03 凌晨 | 调研 | Model A 确认 |
| 2026-07-03 午后 | Phase 0 | Python 参考实现 + c1ea7999 验证 PASS |
| 2026-07-03 下午 | 文档 | PRD + TAURI_RUST_PLAN 改写为只读查看器范围 |
| 2026-07-03 下午 | Phase T0+T1 | 工具链 (rustup/pnpm/webkit2gtk) + Tauri scaffold + 3 个 Rust 核心模块端口 (paths/storage/canonical) + 3 测 PASS |
| 2026-07-03 下午 | Phase T2 | React UI (SessionTree/SessionDetail/SourceBadge + Zustand + Tailwind) + 1588 modules Vite build ✓ |
| 2026-07-03 晚 | **v0.1 完工** | **只读 session 查看器可用, 待用户跑 `pnpm tauri dev` 验收** |
| **下一步** | v0.2 启动 | 写 [SYNC_DESIGN.md](SYNC_DESIGN.md) 设计稿, 然后按用户决定加 sync 还是先观察 |

---

## 11. 一句话总结

> bettercursor v0.1 = **cc-switch 风格的 Cursor session 只读查看器**, 用 Tauri + Rust 写.
> 不做同步, 不做 daemon, 不做跨设备 — **只让用户一眼看完全部本机 session 在哪、什么时候更新过、能不能 resume**.
> 1:1 参照 [cc-switch-session.png](cc-switch-session.png): 左树 + 右详情 + 复制 resume 命令.
>
> 后续能力 (v0.2+) 详见 [SYNC_DESIGN.md](SYNC_DESIGN.md).

---

## 12. 技术选型与依赖

### 12.1 已验证的独立能力 (Phase 0, Python 参考)

| 模块 | 行数 | 用途 |
|------|-----|------|
| `adapter/fix_orphan_sessions.py` | 172 | Phase 0 验证, 修 c1ea7999 root |
| `bettercursor/paths.py` | 182 | 4 层路径解析 |
| `bettercursor/storage.py` | 254 | WAL-safe SQLite 读 |
| `bettercursor/blob_dag.py` | 188 | protobuf parser, root 推断 (v0.1 不用, 留 v0.2) |
| `bettercursor/snapshot.py` | 198 | gzip snapshot codec (v0.1 不用) |
| `bettercursor/conflict.py` | 96 | 5-way 冲突 (v0.1 不用) |
| `bettercursor/layer2.py` | 183 | store.db 写入 (v0.1 不用, v0.2 delete 用) |
| `bettercursor/layer3.py` | 189 | state.vscdb 写入 (v0.1 不用) |

**v0.1 Rust 实际需要端口的**: 3 个 (paths + storage + canonical). 其他 5 个等 v0.2 (delete) 时再端口.

### 12.2 选型决策: Tauri + Rust (无 sidecar, 无 daemon)

| 选择 | 理由 |
|------|------|
| **Tauri v2** | 用户明确指定, 体积小 (vs Electron), 原生性能 |
| **纯 Rust 端口** | 用户明确指定, 单一语言, MIT 干净, 不依赖 cursaves (AGPL-3.0 风险) |
| **不引入 tokio / notify / flate2** | v0.1 不监听, 不压缩, 不异步 — 启动扫一次就完事 |
| **不用 sidecar Python** | 用户要 Rust, 引入 Python 运行时违背 |
| **shadcn/ui + Tailwind** | 跟 cc-switch 风格一致, 暗色 + 圆角 + lucide 图标 |
| **React 18 + Vite + TypeScript** | Tauri v2 默认模板, 启动快 |

### 12.3 Cargo.toml 核心依赖 (v0.1)

```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-fs = "2"
tauri-plugin-shell = "2"
tauri-plugin-clipboard-manager = "2"  # 复制 resume 命令
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

**二进制体积估计**: bundled SQLite + tauri ≈ 5-8 MB (release + lto).

### 12.4 退出策略

| 场景 | 回退 |
|------|------|
| Tauri 在 Linux 装不上 | 改 Electron + TS, 前端 90% 复用 |
| Rust port 慢 | storage.rs 临时用 Python sidecar, 后续替换 |
| shadcn/ui 抄不对 | 直接 `view-source:` 看 cc-switch 类名, 用纯 CSS 重写 |
| 用户改主意加同步 | 端口 `core/layer2.rs` 写函数, 新增 `commands::sync_now`, 不动前端 |

### 12.5 许可证注意

| 目录 | 许可 | 约束 |
|------|------|------|
| `vendored/cursaves/` | **AGPL-3.0** | 仅作源码参考; **不** install / import / 放入 `sys.path` / cargo workspace |
| `vendored/cursor-history/` | MIT | 仅作源码参考; **不** npm 依赖 / 运行时耦合 |
| bettercursor 本体 | **MIT** | — |

- 两个 vendored 子目录均保留各自 LICENSE, 标注「仅作源码参考, 非运行时依赖」
- 可借鉴算法索引与优先级见 [SYNC_DESIGN.md §11.5](SYNC_DESIGN.md); 代际关系见 [BACKGROUND.md §12](BACKGROUND.md)