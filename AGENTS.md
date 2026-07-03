# bettercursor 仓库规范(单一 Tauri 项目)

> 本文件是仓库级通用规范。bettercursor 是 **单项目** 仓库(非 monorepo),不存在子项目覆盖场景;
> 如未来引入子目录或 workspace,按 "就近覆盖" 原则(子目录 AGENTS.md 优先)再扩展。

## 仓库结构

```
bettercursor/
├── src/                     # React 19 + TypeScript 前端 (Vite + Tailwind + Zustand 5)
├── src-tauri/               # Rust 后端 + Tauri 配置 (src-tauri/src/core/ 为合并层)
├── tests/                   # Python 兼容性 / parity 测试
├── bettercursor/, adapter/  # 旧版 Python 守护进程(存档,不维护,仅做算法参考)
├── vendored/                # 上游 Cursor 解析库快照(子仓库,只读参考)
├── README.md  PRD.md  SYNC_DESIGN.md  TAURI_RUST_PLAN.md  BACKGROUND.md  goal.md
```

### 关键目录职责

- `src-tauri/src/core/canonical.rs` —— 三层 Cursor 存储 (Layer 1 JSONL / Layer 2 store.db / Layer 3 state.vscdb) 的合并唯一源。修改本文件即修改 Rust→TS 的 contract,前端 `src/lib/types.ts` 需同步。
- `src-tauri/src/core/paths.rs` —— 4 层用户目录解析 (mac / linux_desktop / linux_cli / fallback)。这是 **唯一允许** 知道 Cursor 配置目录在哪的模块。
- `src-tauri/src/core/storage.rs` —— WAL-safe SQLite reader(必须通过这里读 vscdb / store.db,不要直接打开)。
- `src/lib/tauri.ts` —— 前端 invoke / listen 的 typed wrapper,所有 IPC 调用必须经过这里导出。
- `src-tauri/capabilities/default.json` —— Tauri ACL 白名单。新增 plugin 必须在 `permissions` 数组里 opt in,否则前端调用 `undefined.invoke` TypeError。

## 通用约定

- 所有对外回复、代码注释、文档字符串、字段描述统一使用 **简体中文**;**代码标识符** 保持英文规范命名。
- 优先沿用现有架构、模块边界和命名风格,避免为局部需求引入跨模块重构。
- 只修改与当前任务直接相关的文件,不要回退用户或其他人未要求的改动。
- **设计文档优先**: 涉及功能、接口、数据结构、流程或业务规则的非平凡改动,先编辑 `PRD.md` / `SYNC_DESIGN.md` / `TAURI_RUST_PLAN.md` 等设计文档明确边界,再动手改代码。Bugfix / 局部样式 / 重命名等小改动不必走这一步。
- 在动手修改前,先确认最近的实现、测试约定和附近代码路径,再做 **最小、可验证** 的变更。
- **协议级变更**(`canonical.rs` / `paths.rs` / 任何 Tauri 命令签名)必须同步更新 `src/lib/types.ts` 与 `src/lib/tauri.ts`,并新增/更新对应 Rust 单测(`#[cfg(test)] mod tests`)。

## 测试与验证

- **Rust 单测**: `cargo test --manifest-path src-tauri/Cargo.toml canonical` —— 任何动 `canonical.rs` / `paths.rs` / `storage.rs` 的改动都必须追加或更新单元测试。
- **TS 类型**: `pnpm exec tsc --noEmit` 必须通过才能提交。
- **运行**: `pnpm tauri dev` 启动开发模式。任何 Rust 改动会自动 rebuild;`src/` 改动通过 Vite HMR 推送。Qt/Wayland 环境需要环境变量降级,见 README。
- **不要** 在没有验证的情况下口头声称 "测试通过"。要么贴 cargo test 输出,要么就写代码并截图/展示日志。

## 提交与协作

- 提交信息使用 **中文**,推荐 emoji 前缀:
  - `✨ feat` — 新功能 / 新面板
  - `🐛 fix` — bug 修复
  - `🛠️ refactor` — 重构(不改外部行为)
  - `📄 docs` — 文档改动
  - `✅ test` — 仅测试改动
- 当前阶段 (`v0.x`) 仓库为单人 early-stage,**直接 push 到 `main`**。不允许 force-push / rebase 已发布的历史。
- 引入多人协作 / 外部贡献前,需要先迁移到 enenzuo 同款的 `main` + `dev` 双分支模型 + PR 流程。
- 合并完成的 milestone 不创建 `*-old` / `*-archive` 之类的封存分支,改用 git tag 标在合并提交上。

## Tauri 关键坑(写入仓库规格层)

> 这些坑在 README "踩坑记录" 部分有冗余描述;这里列出 **每次重构前必须自检** 的硬性约束:

1. **Tauri 2 capabilities ACL 是白名单制。** 加 `plugin:foo|bar` 后必须同步在 `src-tauri/capabilities/default.json` 的 `permissions` 里加 `foo:default` 或 `foo:allow-bar`,否则前端表现为 `undefined.invoke` TypeError。
2. **WebKitGTK devtools 必须 opt-in。** `src-tauri/Cargo.toml` 里的 `tauri = { version = "2", features = ["devtools"] }` 删掉就没 Inspect 入口。
3. **`String::truncate` 不能在 UTF-8 非 char-boundary 上调用。** 处理含 CJK / emoji 的字符串时务必走 `truncate_to_char_boundary` helper(已在 `canonical.rs` 定义)。Rust 单测 `indexable_does_not_panic_on_multibyte` 是这个坑的回归保护,**不得删除**。
4. **React 19 + Zustand 5 的无限 re-render。** 衍生状态(`groupSessionsByProject` 等含 `[...arr].sort()` 的函数)必须用 `useMemo` 在组件层 memoize,**不要** 在 selector 里用 `useShallow` —— 它只看一层 ref,nested array 上必崩。详见 `src/store/sessionStore.ts` 注释。
5. **Wayland / GPU 兼容性。** `pnpm tauri dev` 在部分 compositor 下黑屏,需要 `WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 LIBGL_ALWAYS_SOFTWARE=1` 降级。README 给完整降级方案。

## 文档索引

| 文件 | 内容 |
|---|---|
| [README.md](README.md) | 入门 / 功能状态 / 项目结构 / 踩坑记录 |
| [PRD.md](PRD.md) | 产品需求 v0.1 功能矩阵 + 验收标准 |
| [SYNC_DESIGN.md](SYNC_DESIGN.md) | v0.2+ 同步功能(本地 + 跨设备) |
| [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) | Python → Rust 模块映射 + Cargo 依赖清单 |
| [BACKGROUND.md](BACKGROUND.md) | 项目历程(Python 守护进程 → Tauri 重构) |
| [goal.md](goal.md) | 原始 brief |

## Claude / Copilot 兼容入口

- Claude → [`CLAUDE.md`](CLAUDE.md)(shim,正规范即为本文)
- GitHub Copilot → [`.github/copilot-instructions.md`](.github/copilot-instructions.md)(shim,正规范即为本文;若仓库内没有该文件,规则照本文执行)
