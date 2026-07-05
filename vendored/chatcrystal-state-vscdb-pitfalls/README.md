# ChatCrystal — state.vscdb 解析踩坑记 (快照)

> **只读参考**. 非代码依赖, 不进 runtime / Cargo / npm.

| 字段 | 值 |
|------|-----|
| 原标题 | Cursor 的 state.vscdb 解析踩坑记 |
| 来源 | https://jishuzhan.net/article/2055923341928271873 |
| 关联项目 | [ChatCrystal](https://github.com/ZengLiangYi/ChatCrystal) (文内提及) |
| 快照日期 | 2026-07-05 |
| 许可 | 网页文章; 快照仅供 bettercursor 团队离线考古, 不声称版权 |

## 本目录内容

- [`ARTICLE.md`](ARTICLE.md) — 文章正文 Markdown 快照 (由 WebFetch 抓取整理, 已去掉站点导航/推荐区)
- 本 README — 元数据 + 与 bettercursor 的对照说明

## 与 bettercursor 的关系

| 文章「坑」 | bettercursor 现状 |
|-----------|-------------------|
| 工作区 DB 索引 + 全局 DB bubble 正文 | ✅ `scan_layer3_into` 读 workspace + global |
| `ItemTable` vs `cursorDiskKV` | ✅ `storage.rs` 分表读 |
| `bubbleId:<composerId>:<bid>` 前缀匹配 | ✅ `list_keys("bubbleId:{uuid}:")` |
| 空 assistant bubble (流式中间态) | ⚠️ merge 读路径有过滤; 未单独 `_v` warning |
| 孤立 composer (删 workspace 后 bubble 仍在) | ✅ global `bubbleId:` / `composerData:` 扫描 |
| thinking 多格式 | ✅ `extract_l3_bubble_text` |
| workspace.json URL 编码 | ✅ project path 解析 |
| **未写 L2 store.db / CLI 栈** | bettercursor 额外覆盖 L1+L2; 此文仅 Desktop L3 子集 |

完整交叉索引: [SYNC_DESIGN.md §11.6](../../SYNC_DESIGN.md).
