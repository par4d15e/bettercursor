# vibe-replay — Cursor 本地存储深度文 (快照)

> **只读参考**. 非代码依赖, 不进 runtime / Cargo / npm.

| 字段 | 值 |
|------|-----|
| 原标题 | What Does Cursor Store on Your Machine? A Deep Dive into ~/.cursor/ and state.vscdb |
| 来源 | https://vibe-replay.com/blog/cursor-local-storage/ |
| 作者 / 项目 | [vibe-replay](https://vibe-replay.com/) |
| 快照日期 | 2026-07-05 |
| 许可 | 网页文章; 快照仅供 bettercursor 团队离线考古, 不声称版权 |

## 本目录内容

- [`ARTICLE.md`](ARTICLE.md) — 文章正文 Markdown 快照 (由 WebFetch 抓取整理)
- 本 README — 元数据 + 与 bettercursor 的对照说明

## 与 bettercursor 的关系

| 文章观点 | bettercursor 现状 |
|---------|-------------------|
| 三层主存储: `chats/store.db` + JSONL + global `state.vscdb` | ✅ 与 `paths.rs` L1/L2/L3 一致 |
| store 栈 vs composer 栈 ID 池可能不相交 | ✅ §2.5 Q6; **有效 CLI 会话 L1↔L2 同 uuid** |
| `composerData` + `bubbleId` 为主 replay 轴 | ✅ `scan_layer3_into` + `read_layer3_bubbles` |
| `agentKv` 为 request/provenance 轴, 非主列表源 | ✅ 读 UI 不依赖; v0.3.0 最小写入 |
| checkpoint / ai-tracking / prompt_history 等旁路 | ❌ 未 ingest (scope 外) |

完整交叉索引: [SYNC_DESIGN.md §11.6](../../SYNC_DESIGN.md).
