# Cursor 的 state.vscdb 解析踩坑记

> 原文: https://jishuzhan.net/article/2055923341928271873  
> 快照: 2026-07-05 (bettercursor vendored 参考, 非官方 Cursor spec)  
> 关联项目: ChatCrystal — https://github.com/ZengLiangYi/ChatCrystal

本文面向：想了解 SQLite 数据库逆向工程细节的开发者，或在解析 VS Code 系 IDE 数据时遇到问题的人。

---

## 为什么要写这篇

Cursor 的对话数据存在 SQLite 数据库里，但这个数据库没有官方文档，没有 schema 说明，甚至表名都不按常理出牌。ChatCrystal 在实现 Cursor 适配器时踩了不少坑，这篇把关键的几个记下来。

## 坑 1：两个数据库，数据分散

Cursor 把数据拆成了两个：

```
~/.config/Cursor/User/
├── workspaceStorage/<hash>/state.vscdb   ← Composer 元数据（ID、时间、项目路径）
└── globalStorage/state.vscdb             ← 对话内容（Bubble 数据）
```

元数据和内容不在同一个库里。

工作区数据库存了「这个项目有哪些对话」，全局数据库存了「每条对话说了什么」。你需要先从工作区拿到 composerId，再去全局库里查对应的 bubble。

```
工作区 DB:
  composer.composerData → {allComposers: [{composerId: "abc", createdAt: ...}]}

全局 DB:
  cursorDiskKV → key: "bubbleId:abc:001" → value: {type: 1, text: "你好"}
  cursorDiskKV → key: "bubbleId:abc:002" → value: {type: 2, text: "你好！有什么..."}
```

## 坑 2：表名是 camelCase

VS Code 系 IDE 的 SQLite 数据库里，表名是 `ItemTable`（PascalCase），但 Cursor 的全局 KV 存储用的是 `cursorDiskKV`（camelCase）。

```sql
-- 工作区 DB：标准 VS Code 表名
SELECT value FROM ItemTable WHERE [key] = 'composer.composerData';

-- 全局 DB：Cursor 自定义表名
SELECT [key], value FROM cursorDiskKV WHERE [key] LIKE 'bubbleId:abc:%';
```

如果你用操作 `ItemTable` 的代码去查 `cursorDiskKV`，会得到 "no such table" 错误。

## 坑 3：key 格式是多段冒号分隔

全局库里 bubble 的 key 格式是：

```
bubbleId:<composerId>:<bubbleId>
```

比如 `bubbleId:abc123:def456`。需要用 `LIKE` 前缀匹配：

```sql
SELECT [key], value FROM cursorDiskKV WHERE [key] LIKE 'bubbleId:abc123:%'
```

从 key 里提取 composerId：

```sql
SELECT DISTINCT SUBSTR([key], 10, INSTR(SUBSTR([key], 10), ':') - 1)
FROM cursorDiskKV WHERE [key] LIKE 'bubbleId:%'
```

`SUBSTR([key], 10)` 跳过 `bubbleId:` 前缀（9 个字符 + 冒号 = 从第 10 位开始）。

## 坑 4：schema 版本 _v 可能更新

Bubble 数据里有一个 `_v` 字段表示 schema 版本：

```json
{
  "_v": 3,
  "type": 1,
  "text": "帮我看看这段代码"
}
```

目前 `_v: 3` 是主流版本。Cursor 更新后可能会变成 `_v: 4`、`_v: 5`。ChatCrystal 在解析时会检查版本号，遇到未知版本只打 warning，不会崩溃：

```typescript
if (bubble._v && bubble._v > 3) {
  console.warn(`[Cursor] Unknown bubble schema version: ${bubble._v}`);
}
```

## 坑 5：空助手消息是流式中间态

很多 assistant 类型的 bubble 的 `text` 是空的：

```json
{"_v": 3, "type": 2, "text": "", "createdAt": "2026-05-10T10:30:01Z"}
```

这些是流式传输的中间状态。Cursor 在 AI 回复过程中会不断创建新的 bubble，先把空壳写进去，再逐步填充内容。

ChatCrystal 直接跳过空的 assistant bubble：

```typescript
if (msgType === "assistant" && !text) continue;
```

用户消息不需要这个过滤，因为用户消息是完整写入的。

## 坑 6：孤立对话不会丢失

删掉了项目目录后，工作区数据库跟着没了，但全局数据库里的 bubble 数据还在。

处理方式是：

1. 扫描全局库里所有 `bubbleId:*` 的 key
2. 提取出所有 composerId
3. 过滤掉已经在工作区列表里的
4. 对剩余的 composerId，检查是否有至少一条有文本内容的 bubble
5. 有内容的就导入，项目名显示为空

```typescript
const result = db.exec(
  "SELECT DISTINCT SUBSTR([key], 10, INSTR(SUBSTR([key], 10), ':') - 1) FROM cursorDiskKV WHERE [key] LIKE 'bubbleId:%'"
);

const candidates = result[0].values
  .map(r => r[0] as string)
  .filter(id => !knownIds.has(id));

for (const composerId of candidates) {
  const bubbles = db.exec(
    `SELECT value FROM cursorDiskKV WHERE [key] LIKE 'bubbleId:${composerId}:%' LIMIT 20`
  );
  const hasContent = bubbles[0].values.some(row => {
    const bubble = JSON.parse(row[0] as string);
    return (bubble.text || "").trim().length > 0;
  });
  if (hasContent) valid.push(composerId);
}
```

`LIMIT 20` 是优化 — 只要找到一条有内容的就够了。

## 坑 7：thinking 块有多种格式

```json
{
  "allThinkingBlocks": [
    {"thinking": "让我分析一下这段代码..."},
    {"text": "这是一个递归函数..."}
  ]
}
```

有的块用 `thinking` 字段，有的用 `text` 字段：

```typescript
if (bubble.allThinkingBlocks && bubble.allThinkingBlocks.length > 0) {
  const thinkingTexts = bubble.allThinkingBlocks
    .map(b => b.thinking || b.text || "")
    .filter(Boolean);
  if (thinkingTexts.length > 0) {
    thinking = thinkingTexts.join("\n");
  }
} else if (bubble.thinking) {
  thinking = bubble.thinking;
}
```

## 坑 8：workspace.json 的路径编码

```json
{"folder": "file:///c%3A/Users/Rayner/Project/MyApp"}
```

需要先去掉 `file:///` 前缀，再 `decodeURIComponent` 解码：

```typescript
const rawFolder = wsJson.folder || "";
const folder = decodeURIComponent(rawFolder.replace(/^file:\/\/\//, ""));
```

## 总结

| 坑 | 解决方案 |
| --- | --- |
| 两个数据库 | 先查工作区拿 ID，再查全局拿内容 |
| 表名不一致 | 工作区用 `ItemTable`，全局用 `cursorDiskKV` |
| key 多段冒号 | `LIKE 'bubbleId:%'` 前缀匹配 |
| schema 版本变化 | 检查 `_v`，未知版本打 warning 不崩溃 |
| 空助手消息 | 跳过 text 为空的 assistant bubble |
| 孤立对话 | 扫描全局库所有 bubbleId，过滤已知的 |
| thinking 格式不统一 | 同时检查 `thinking` 和 `text` 字段 |
| 路径 URL 编码 | `decodeURIComponent` + 去掉 `file:///` |

---

**注:** 本文仅覆盖 Desktop L3 (workspace + global vscdb), **未涉及** `~/.cursor/chats/` Layer 2 store.db / CLI 栈。bettercursor 三层模型见仓库 `SYNC_DESIGN.md §2.5 Q6`。
