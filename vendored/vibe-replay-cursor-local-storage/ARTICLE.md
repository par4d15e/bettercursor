# What Does Cursor Store on Your Machine? A Deep Dive into ~/.cursor/ and state.vscdb

> 原文: https://vibe-replay.com/blog/cursor-local-storage/  
> 快照: 2026-07-05 (bettercursor vendored 参考, 非官方 Cursor spec)

---

Run this right now:

```bash
du -sh ~/.cursor ~/Library/Application\ Support/Cursor 2>/dev/null
```

On my Mac, that prints:

| Path | Size |
| --- | --- |
| `~/.cursor` | 2.1 GB |
| `~/Library/Application Support/Cursor` | 4.9 GB |

One important caveat up front: this is a local audit of one heavily used macOS machine, not an official Cursor storage spec.

The exact sizes and counts on your machine will differ. The useful part is the shape of the system.

I expected Cursor to have one obvious "session log" folder, the way Claude Code has `~/.claude/projects/*.jsonl`.

It doesn't.

What I found instead was a layered system:

- SQLite chat databases in `~/.cursor/chats/`
- transcript JSONL files in `~/.cursor/projects/.../agent-transcripts/`
- a massive global state database at `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
- workspace state DBs, local file history, checkpoint diffs, and even separate AI-tracking tables on top of that

If you're building tooling on top of Cursor session data, this matters. If you're just curious where your chats went, it matters even more.

---

## Where are Cursor sessions actually stored?

On this machine, Cursor session data is spread across three primary sources.

### 1. ~/.cursor/chats/*/*/store.db

This is the cleanest "real chat database" layer.

I found 171 local `store.db` files under `~/.cursor/chats/`, totaling about 280 MB of actual SQLite payload.

Metadata fields like:

- `agentId`
- `latestRootBlobId`
- `name`
- `mode`
- `createdAt`
- `lastUsedModel`

Sample session metadata:

```json
{
  "agentId": "d5c2d589-344b-4f62-a091-af4701f742ce",
  "name": "Cursor Session TTL",
  "mode": "auto-run",
  "lastUsedModel": "gpt-5.4-high"
}
```

This is not "some cache." It's a real local conversation store.

### 2. ~/.cursor/projects/*/agent-transcripts/*.jsonl

This is the most Claude-like layer.

I found:

- 138 transcript JSONL files
- 17 `agent-tools/*.txt` sidecar files

These transcripts can be flat:

```
agent-transcripts/<session-id>.jsonl
```

or nested:

```
agent-transcripts/<session-id>/<session-id>.jsonl
```

This is the easiest source to inspect by hand. It often contains the user-visible conversation text, and in some flows it also preserves image references and tool markers.

But it is not the whole story. On its own, it is incomplete.

### 3. ~/Library/Application Support/Cursor/User/globalStorage/state.vscdb

This is where things get wild.

On my machine, `state.vscdb` alone is 1.24 GB.

And it doesn't just hold preferences. It holds large volumes of chat/composer state in a key-value table called `cursorDiskKV`.

Biggest key families:

| Prefix | Count | Approx size |
| --- | --- | --- |
| `agentKv` | 88,826 | 506.5 MB |
| `bubbleId` | 55,889 | 463.9 MB |
| `composerData` | 1,188 | 45.4 MB |
| `checkpointId` | 5,842 | 42.5 MB |
| `messageRequestContext` | 1,786 | 23.8 MB |

Not every big key family is equally useful for replay:

- `composerData` and `bubbleId` are the most obviously replay-relevant
- `messageRequestContext` looks more like prompt-building context snapshots
- `checkpointId` looks like restore / inline-diff state
- `agentKv` appears to be a separate message/blob store that is often tagged with request IDs

`messageRequestContext::<uuid>` does appear to share its first UUID with `composerData` / checkpoint session IDs — a per-session context sidecar.

**Important nuance:** Cursor does not appear to use one universal session UUID across all of these stores.

On this machine, the `store.db` session IDs and the `composerData` session IDs were disjoint. The transcript layer sat across both:

- 106 transcript IDs matched `store.db` sessions
- 24 transcript IDs matched `composerData` sessions
- 0 matched both

Mental model: Cursor has at least two replay stacks, and transcript JSONL can attach to either one.

---

## Cursor doesn't have one transcript format. It has a local storage stack.

With Claude Code:

```
session = one JSONL file
```

With Cursor:

```
store-backed session = store.db + optional transcript JSONL
composer-backed session = composerData + bubbleId + optional transcript JSONL + context / checkpoint sidecars
request-scoped provenance = agentKv + checkpoint metadata + ai_code_hashes
```

---

## How the pieces actually connect

- one replay stack built around `store.db`
- another replay stack built around `composerData` + `bubbleId`
- a transcript layer that can attach to either stack
- a separate request/provenance axis built around request IDs

In practice:

- `store.db` session IDs can line up with transcript JSONL IDs
- `composerData:<uuid>` lines up cleanly with `bubbleId:<uuid>:<bid>`
- the first UUID in `messageRequestContext::<uuid>` appears to line up with composer session IDs
- checkpoint `metadata.json.agentRequestId`, `agentKv.providerOptions.cursor.requestId`, and `ai_code_hashes.requestId` line up with each other

Request IDs are not the same thing as the main replay session IDs.

---

## What is inside composerData and bubbleId?

- `composerData:<uuid>`
- `bubbleId:<uuid>:<bid>`

Key field for replay: `fullConversationHeadersOnly`.

Sample:

```json
{
  "name": "Checking Retool Version on Helm",
  "isAgentic": true,
  "fullConversationHeadersOnly": [
    { "bubbleId": "5d29a280-...", "type": 1 },
    { "bubbleId": "08f6cd6c-...", "type": 2, "serverBubbleId": "8f397408-..." }
  ]
}
```

Bubble fields can include: `text`, `tokenCount`, `images`, `toolFormerData`, `pullRequests`, `relevantFiles`, `recentlyViewedFiles`, `thinkingDurationMs`, `errorDetails`.

---

## Request-level message blobs (agentKv)

Over 32,000 readable `agentKv:blob:*` payloads with `role`, `content`, sometimes `providerOptions.cursor.requestId`.

Not the main replay source yet — sits on request/provenance axis.

---

## Prompt history

`~/.cursor/prompt_history.json` — rolling plain strings (500 entries on author's machine), not Claude Code-style structured history.

---

## Checkpoints

`~/Library/Application Support/Cursor/User/globalStorage/anysphere.cursor-commits/checkpoints/`

Each checkpoint: `metadata.json`, `diffs/`, `files/`. Request-scoped via `agentRequestId`.

VS Code local history: `~/Library/Application Support/Cursor/User/History/` — file recovery, not chat.

---

## AI tracking database

`~/.cursor/ai-tracking/ai-code-tracking.db` — `ai_code_hashes`, `scored_commits`, etc. Attribution layer, not main chat store.

---

## Local retention

No evidence of fixed 30-day TTL on author's machine; old `store.db` and history persist.

---

## Workspace state

`~/Library/Application Support/Cursor/User/workspaceStorage/*/state.vscdb` — composer UI state, not best replay source of truth.

---

## Core map

```
~/.cursor/chats/
~/.cursor/projects/
~/Library/Application Support/Cursor/User/globalStorage/state.vscdb
```

Supporting layers:

```
~/Library/Application Support/Cursor/User/workspaceStorage/
~/Library/Application Support/Cursor/User/History/
~/Library/Application Support/Cursor/User/globalStorage/anysphere.cursor-commits/checkpoints/
~/.cursor/prompt_history.json
~/.cursor/ai-tracking/ai-code-tracking.db
```

---

## Inspect commands

```bash
du -sh ~/.cursor ~/Library/Application\ Support/Cursor 2>/dev/null
find ~/.cursor/chats -name store.db 2>/dev/null | wc -l
find ~/.cursor/projects -name '*.jsonl' -path '*/agent-transcripts/*' 2>/dev/null | wc -l
find ~/.cursor/projects -name '*.txt' -path '*/agent-tools/*' 2>/dev/null | wc -l
```

Interactive: `npx vibe-replay`

---

**Headline:** Cursor stores a lot more locally than its UI reveals — a stack of overlapping systems for replay, recovery, request context, and attribution.
