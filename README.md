# bettercursor

> Local **Cursor** session viewer (read-only). **Tauri 2 + React 19 + Rust**, UI inspired by [cc-switch](https://github.com/farion1231/cc-switch).
>
> 🌐 [English](README.md) · [简体中文](README.zh-CN.md)

![status](https://img.shields.io/badge/status-v0.3.5-success)
![platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue)
![stack](https://img.shields.io/badge/Tauri-2-orange)
![language](https://img.shields.io/badge/Rust-1.77%2B-orange)
![i18n](https://img.shields.io/badge/i18n-zh--CN%20%7C%20en-green)
![sync](https://img.shields.io/badge/sync-Transport%20trait%20v1-purple)

## What it is

`bettercursor` is a desktop app that **views** every AI conversation
Cursor IDE stores on disk. It scans the three SQLite + JSONL layers under
`~/.config/Cursor` (Linux) / `~/Library/Application Support/Cursor` (macOS),
deduplicates across layers, and renders a single merged session list.

Design goals:
- **Read-only by default** — v0.2.1+ added opt-in writes (`fix_orphans` /
  `delete_session` / `sync_session_layer23`) but the app never touches
  Cursor's working files except through these explicit commands
- **cc-switch UI** — left project-grouped tree + right conversation detail
- **Byte-identical to a Python reference implementation** — MD5 `chat_root`
  parity tests pass

## Feature status

### v0.3.5 (✅ current, shipped 2026-07-05)

- [x] **Optional L3 soft delete** — Desktop-aligned sidebar archive +
      purge `bubbleId` / `checkpointId` rows; keep `composerData` shell
- [x] **Subagent sessions** — read L2 `meta[0].subagentInfo`; nest under
      `rootParentAgentId` in the sidebar tree; collapsed by default
- [x] **Hide empty Desktop ghosts** — `Untitled · uuid`, zero bubbles,
      no CLI source: filtered out of the session list (disk untouched)
- [x] **Conversation read fixes** — L3 header chain + L2 enrichment;
      trim bad L3 prefixes; strip context envelopes / `[REDACTED]`
- [x] **Delete tombstones** — `deleted_sessions` stops L3-only rows from
      resurrecting after bettercursor delete

### v0.3.4 (2026-07-05)

- [x] **L2→L3 bubble enrichment** — `layer2_messages` walks the CLI
      `store.db` DAG and replaces L1 `[REDACTED]` stubs with full
      assistant text + tool-call metadata before `bubbleId` inject
- [x] **User image attachments** — L2 `image` blobs decode to
      `images[]` data URLs on user `bubbleId` rows (Desktop replay)
- [x] **Re-inject detection** — sessions with CLI envelopes, redacted
      assistant text, or missing images trigger Layer 3 rewrite
- [x] **补 Layer 3 playbook** — see [SYNC_DESIGN §0.5](SYNC_DESIGN.md)
      (quit Cursor → sync in bettercursor → restart Cursor)

### v0.3.2 (2026-07-05)

- [x] **`<SettingsDialog>`** — gear icon in the sidebar header; consolidates
      language switch, cross-device sync (`<SyncPeersPanel>`), and conflict
      resolution (`<ConflictResolvePanel>`)
- [x] **i18n fix** — merged duplicate `sync` keys in locale JSON (status badge
      no longer shows raw `sync.autoSync`)
- [x] **Dark-theme language switcher** — segmented buttons replace native
      `<select>`; `color-scheme: dark` on root
- [x] **Sidebar polish** — product name `BetterCursor` in header; collapse/expand
      all project groups; removed non-functional back button
- [x] **Conflict UX** — neutral copy + badge on settings when conflicts pending

### v0.3.1 (2026-07-05)

- [x] **LAN cross-device sync** — mDNS discovery, 6-digit pairing, trusted peers,
      outbox, background sync loop
- [x] **`<SyncPeersDialog>` / `<ConflictResolveDialog>`** — shipped in v0.3.1;
      **superseded by `<SettingsDialog>` in v0.3.2**

### v0.3.0 (2026-07-05)

- [x] **`~/.bettercursor/unified.db`** (PR-1): 8 tables + FTS5 +
      `rebuild_from_cursor_state` + archive / conflicts / sync_runs
- [x] **pre-PR-2 read-path parity**: full L3 bubble text extraction,
      Cursor 3.0+ session discovery, timestamp gap fill,
      cursor-history parity fixtures
- [x] **snapshot codec v4** (`core/snapshot.rs`): bubble-level JSON;
      push still uses 8-field `snapshot_meta`
- [x] **Conflict 5-way** (`core/conflict.rs`): classify / bubble_diff /
      auto_merge; `transport_pull` writes back into unified.db
- [x] **Transport async** (`tokio` + `async-trait`); Tauri commands
      stay sync, backend uses `block_on`
- [x] **agentKv minimal slice**: `write_layer3` copies agent blobs
      referenced by `conversationState`
- [x] **126 Rust unit tests** (`cargo test --lib`)

Inspect unified.db:

```bash
sqlite3 ~/.bettercursor/unified.db "SELECT uuid, bubble_count, content_hash FROM sessions LIMIT 5;"
```

### v0.2.6 (shipped 2026-07-04)

- [x] **Cross-device sync — Transport trait first cut**:
      `core::transport::Transport` trait (4 methods: `push` / `pull` /
      `list_remote` / `endpoint_id`, **sync** — deliberately diverging
      from the `async_trait` in [SYNC_DESIGN §4.4](SYNC_DESIGN.md#4-transport-trait)
      until v0.3.0). One impl: `SshRsyncTransport` (T2), shelling out
      to system `ssh` / `rsync` (no new Cargo deps, no `tokio`,
      no `russh`).
- [x] **Minimum v0.2.6 snapshot carrier**: `SessionSnapshot`
      (8 metadata fields — uuid / `last_updated_at_ms` / host /
      `project_slug` / `project_path` / `source_path` / `text_preview`
      capped 280 chars / `bubble_count`). No bubbles / blobs yet —
      that's v0.3.0 unified.db territory.
- [x] **Peer config file**: `~/.bettercursor/transports.json`
      (separate from the prefs `config.json`). Atomic save
      (`*.tmp` + rename).
- [x] **4 Tauri commands**: `transport_list_peers` /
      `transport_test` / `transport_push` / `transport_pull`. Plus
      4 typed IPC wrappers in `src/lib/tauri.ts` (`PeerSummary` /
      `TestReport` / `PushReport` / `PullReport` / `RemoteSession`).
- [x] **No UI yet** — usage is via `invoke('transport_*')` from dev
      console + manually editing `transports.json`. SyncPeersDialog is
      a v0.3.0 milestone.
- [x] **20 Rust unit tests** for snapshot codec, config serde,
      `ssh_cmd` safety flags, push failure stderr, etc. Plus
      `tests/fixtures/fake-{ssh,rsync}.sh` mock binaries for
      CI-friendly testing without a real SSH peer.
- [x] **v0.2.6 housekeeping** (shipped together): CI matrix gains
      `macos-13` (Intel x64 dmg alongside Apple Silicon dmg), Node
      20 → 22, vitest 2 + jsdom 25 + `@testing-library/react` 16 +
      15-case test suite for `<SyncStatusBadge>` / `<BrokenBadge>`
      i18n-aware fallback. Zero business-code change.

### v0.2.5 (shipped 2026-07-04)

- [x] **Cross-platform packaging**: Linux `.deb` / `.AppImage` + macOS
      unsigned `.dmg` (macOS 10.15+, Apple Silicon) + Windows `.msi` /
      `.exe` (NSIS). All built by GitHub Actions matrix
      ([`release.yml`](.github/workflows/release.yml))
- [x] **i18n (zh-CN / en)**: react-i18next + `src/locales/{zh-CN,en}.json`
      (~110 UI strings) + `<LanguageSwitcher>` header `<select>` +
      localStorage persistence (`i18nextLng`)
- [x] Three-file version bump: `package.json` / `Cargo.toml` /
      `tauri.conf.json` all on `0.2.5`; `productName: "BetterCursor"`
      (PascalCase for Mac `.app`)
- [x] Background sync loop wrap-up (v0.2.3): `<SyncNowButton>` (instant
      rescan) + `<SyncStatusBadge>` ("● Auto-sync · 12s ago", 1Hz tick +
      5s backend poll)
- [x] Conversation record expansion (v0.2.2): L1+L2+L3 three-way merge +
      `<MessageList>` thin wrapper
- [x] Repair orphan + delete session (v0.2.1): native `<dialog>` confirm
- [x] Scans 3 Cursor layers on startup (Layer 1 JSONL / Layer 2
      `store.db` / Layer 3 `state.vscdb`)
- [x] Cross-layer dedup; project grouping; full-text search by name /
      project / content / UUID
- [x] MD5 `chat_root` byte-identical to the Python reference

### v0.3.2+ (planned, see [SYNC_DESIGN.md](SYNC_DESIGN.md))

- [ ] T3/T4/T5 adapters: git / S3 / Tailscale

## Download & install

Every git tag (`v*.*.*`) triggers
[`.github/workflows/release.yml`](.github/workflows/release.yml) to build
on three platforms. Artifacts end up on the
[Releases](../../releases) page.

### Linux

```bash
# Debian / Ubuntu (.deb — declares libwebkit2gtk-4.1 / libgtk-3 /
# libayatana-appindicator3 in Depends:)
sudo dpkg -i BetterCursor_0.2.6_amd64.deb
sudo apt-get install -f   # satisfy missing deps if dpkg complains

# Portable AppImage (no install, but first run downloads linuxdeploy
# binaries from tauri-apps/binary-releases — needs network)
chmod +x BetterCursor_0.2.6_amd64.AppImage
./BetterCursor_0.2.6_amd64.AppImage
```

### macOS

1. Download `BetterCursor_0.2.6_aarch64.dmg` (Apple Silicon) **or**
   `BetterCursor_0.2.6_x64.dmg` (Intel). Both are unsigned `.dmg`
   built by the `macos-latest` + `macos-13` CI matrix entries.
2. Mount, drag `BetterCursor.app` into `/Applications`.
3. **Bypass Gatekeeper for an unsigned app** (one-shot, cleaner than
   right-click → Open):

   ```bash
   xattr -dr com.apple.quarantine /Applications/BetterCursor.app
   ```

   `com.apple.quarantine` is the extended attribute Finder drops on
   anything downloaded from the internet. `-dr` recurses through the
   whole `.app` bundle (including nested binaries and frameworks) and
   strips every quarantine flag, so subsequent double-clicks work like
   an App Store install.

   Fallback (if `-dr` doesn't stick): right-click `BetterCursor.app`
   → Open → Open. Same effect, but **you'll have to repeat for every
   new build**.

   Sweep everything in /Applications in one go:

   ```bash
   find /Applications -name "*.app" -exec xattr -dr com.apple.quarantine {} \; 2>/dev/null
   ```

### Windows

```powershell
# .msi (MSI installer — good for managed deployment)
msiexec /i BetterCursor_0.2.6_x64_en-US.msi

# or .exe (NSIS — better for personal installs)
.\BetterCursor_0.2.6_x64-setup.exe
```

## Quick start (from source)

### Prerequisites

- **Node 20+** + pnpm 9+ (lockfile is `lockfileVersion: '9.0'`)
- **Rust 1.77+** (`rustup install stable`)
- **Linux**: `webkit2gtk-4.1`, `libsoup-3.0`, `libgtk-3`,
  `libjavascriptcoregtk-4.1`, optional `xdg-desktop-portal-gnome`
- **macOS**: Xcode Command Line Tools
- **Windows**: WebView2 runtime (preinstalled on Win 11)

### Install & run

```bash
git clone https://github.com/par4d15e/bettercursor.git
cd bettercursor
pnpm install

# Dev (HMR + WebKit devtools available)
pnpm tauri dev

# Production build
pnpm tauri build
```

On launch a 1280×800 window opens; in the background the app scans
Cursor's storage and renders ~37 sessions (Linux desktop + Linux CLI
+ macOS sources mix, in a typical setup).

### Wayland note

Some compositors need fallback env vars under WebKitGTK:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 \
LIBGL_ALWAYS_SOFTWARE=1 \
pnpm tauri dev
```

## Project layout

```
bettercursor/
├── src/                   # React + TS frontend
│   ├── components/        # SessionTree, SessionDetail, SourceBadge, ...
│   ├── store/             # Zustand store + selectors
│   ├── lib/               # tauri.ts (IPC wrapper), types.ts
│   ├── i18n/              # i18next init (zh-CN, en)
│   ├── locales/           # zh-CN.json / en.json
│   ├── App.tsx · main.tsx · index.css
├── src-tauri/             # Rust backend
│   ├── src/
│   │   ├── core/
│   │   │   ├── paths.rs       # 4-layer path resolution
│   │   │   ├── storage.rs     # WAL-safe SQLite reader
│   │   │   ├── canonical.rs   # three-layer merge
│   │   ├── lib.rs             # Tauri commands + setup
│   │   ├── main.rs
│   ├── capabilities/          # default.json (ACL whitelist)
│   ├── icons/                 # Tauri bundle icons
│   ├── tauri.conf.json
│   ├── Cargo.toml · Cargo.lock
├── tests/                 # Python compatibility tests (parity check)
├── bettercursor/, adapter/# Old Python daemon reference impl (archive)
├── vendored/              # Upstream Cursor parsing library (subrepo)
├── PRD.md · SYNC_DESIGN.md · AGENTS.md · docs/
└── .github/workflows/     # release.yml (3-OS matrix)
```

## Architecture overview

Session reads happen in **three layers** (see [SYNC_DESIGN.md §2.5 Q6](SYNC_DESIGN.md) for UUID identity):

| Layer | Storage | Path (Linux) | Role |
|---|---|---|---|
| **L1** | JSONL | `~/.cursor/projects/<slug>/agent-transcripts/<uuid>/<uuid>.jsonl` | Transcript; CLI + Desktop both write; **same uuid as L2 when CLI session is valid** |
| **L2** | SQLite | `~/.cursor/chats/<md5(cwd)>/<uuid>/store.db` | **CLI only** (`cursor-agent`); Sidebar resume list on CLI |
| **L3** | SQLite KV | `~/.config/Cursor/User/globalStorage/state.vscdb` (`cursorDiskKV`: `composerData:*`, `bubbleId:*`; plus per-workspace `workspaceStorage/*/state.vscdb`) | **Desktop** composer index + bubble bodies |

### Injecting Layer 3 (CLI → Desktop Sidebar)

**Always quit Cursor Desktop first** — writing `state.vscdb` while Cursor
is running can corrupt the WAL. Then in bettercursor open the CLI session
and run **sync Layer 2/3**; restart Cursor. v0.3.4+ rewrites stub bubbles
when it detects CLI envelopes, `[REDACTED]` assistant text, or missing
images that exist in Layer 2. Full playbook: [SYNC_DESIGN §0.5](SYNC_DESIGN.md).

Rust side (`src-tauri/src/core/`) handles:
1. **`paths.rs`** — parse cursor user dir / chat_root MD5 etc.
2. **`storage.rs`** — WAL-safe read: copy to `tempfile::tempdir()`,
   `PRAGMA wal_checkpoint(TRUNCATE)`, then read-only open.
3. **`canonical.rs`** — cross-layer merge, emit `CanonicalSession`.

Tauri commands exposed to the frontend:

| Command | Args | Returns |
|---|---|---|
| `list_sessions` | — | All sessions in the cache |
| `sync_now` | — | `usize` count, fires `sessions-updated` event |
| `get_conversation` | `uuid` | Parsed `Conversation` with merged bubbles + source_path |
| `get_resume_command` | `uuid`, `source` | `open -a Cursor --args --resume <uuid>` or `cursor-agent --resume <uuid>` |
| `sync_session_layer23` | `uuid`, `cwd` | `SyncReport` (wrote_layer2 / wrote_layer3 / skipped / duration_ms) |
| `fix_orphans` | — | `FixOrphansReport` (scanned / fixed / skipped, auto-backups `store.db`) |
| `delete_session` | `uuid`, `cwd`, `slug` | `DeleteReport` (cursor_running / removed_l1 / removed_l2 / skipped_*) |
| `watcher_status` | — | `{ active, dirs[], last_scan_at_ms }` |
| `platform_info` | — | `<os>: <cursor_user_dir>` (debug) |
| `transport_list_peers` | — | `PeerSummary[]` from `~/.bettercursor/transports.json` |
| `transport_test` | `peerId` | `TestReport` (ok / latency_ms / error?) |
| `transport_push` | `uuid`, `peerId` | `PushReport` (uuid / bytes_written / duration_ms) |
| `transport_pull` | `peerId`, `sinceMs?` | `PullReport` (peer_id / count / snapshots[]) |

## Cross-device sync (v0.2.6)

v0.2.6 ships the **Transport trait first cut**. Configure one or more
peers in `~/.bettercursor/transports.json`:

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

Then from devtools console:

```js
await __TAURI__.invoke('transport_list_peers')          // → [{id:"macbook",...}]
await __TAURI__.invoke('transport_test', { peerId: 'macbook' })  // → {ok:true, latency_ms:42}
await __TAURI__.invoke('transport_push', { uuid: '<a session>', peerId: 'macbook' })
// ~/.bettercursor/peers/bettercursor-main/<host>/<uuid>.json now exists on the peer.
await __TAURI__.invoke('transport_pull', { peerId: 'macbook', sinceMs: 0 })
// → { peer_id: "macbook", count: 1, snapshots: [...] }
```

SSH safety flags baked in: `BatchMode=yes` (no interactive prompts) +
`StrictHostKeyChecking=accept-new` (auto-trust new hosts, fail loud
on key mismatch). The `Transport` trait is **sync** (not `async_trait`)
in v0.2.6 — it migrates to async in v0.3.0 when the offline outbox
lands. A `<SyncPeersDialog>` UI is on the v0.3.0 roadmap.

## Pitfalls

### 1. React 19 + Zustand 5 infinite re-render

`useShallow((s) => derived(s))` looks shallow-stable, but when the
derived function does `[...arr].sort(...)` the inner array refs differ
and the comparison falls through, triggering React 19's
`Maximum update depth exceeded` bail-out.

**Fix**: move the derived value out of the selector, memoize in
`useMemo` at the component level:

```ts
// ❌ Don't:
useStore(useShallow((s) => groupByProject(s.items)))

// ✅
const items = useStore((s) => s.items)
const grouped = useMemo(() => groupByProject(items), [items])
```

### 2. Tauri capabilities ACL is minimal by default

Without plugin-specific permissions, a frontend
`invoke('plugin:foo|bar')` is rejected with a TypeError. Add the
permissions explicitly in `capabilities/default.json`:

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

### 3. WebKitGTK devtools off by default

You must opt in via a Cargo feature, otherwise the right-click menu only
shows "Reload", no "Inspect":

```toml
tauri = { version = "2", features = ["devtools"] }
```

### 4. WebKitGTK Wayland black screen

Some compositors (Mutter / Hyprland) crash on the GPU composition path.
Known workaround:

```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 \
WEBKIT_DISABLE_COMPOSITING_MODE=1 \
LIBGL_ALWAYS_SOFTWARE=1 \
pnpm tauri dev
```

### 5. pnpm 9.4+ rejects empty `pnpm-workspace.yaml`

If a `pnpm-workspace.yaml` exists at the repo root (even just for
`allowBuilds`), it must declare a `packages` field. Without it, pnpm
aborts with `ERROR packages field missing or empty`. Add at minimum:

```yaml
packages:
  - "."
```

This bit us on the first `v0.2.5` release run — all matrix jobs failed
in 30 s.

## Docs

| File | Content |
|---|---|
| [PRD.md](PRD.md) | Product requirements v0.1 feature matrix + acceptance criteria |
| [SYNC_DESIGN.md](SYNC_DESIGN.md) | v0.2+ sync capability design |
| [SYNC_DESIGN.md](SYNC_DESIGN.md) | v0.2+ sync / cross-device design |
| [docs/README.md](docs/README.md) | Doc layout; local archive in `docs/local/` (gitignored) |

## Roadmap

```
v0.2.5 (✅ done)  Cross-platform packaging · i18n · background sync ·
                 conversation records · repair orphan · delete
v0.2.6 (✅ now)   Cross-device sync — Transport trait first cut ·
                 SSH/rsync (T2) impl · 4 Tauri commands · Intel dmg
v0.3.0 (✅ done)   ~/.bettercursor/unified.db · snapshot codec v4 ·
                 async Transport · Conflict 5-way
v0.3.1 (✅ done)   LAN mDNS pairing · outbox · sync loop
v0.3.2 (✅ now)   <SettingsDialog> UI polish · i18n fix · sidebar UX
v0.3.3+           T3 Git · T4 S3 · T5 Tailscale adapters
```

## Acknowledgements

- UI paradigm: [farion1231/cc-switch](https://github.com/farion1231/cc-switch)
- Old Python daemon (`bettercursor/`, `adapter/`) provided the parsing
  algorithm reference
- `vendored/cursaves/` (AGPL, read-only) and `vendored/cursor-history/` (MIT,
  read-only) are upstream Cursor parsing library snapshots; borrowable algorithms
  are indexed in [SYNC_DESIGN.md §11.5](SYNC_DESIGN.md)

---

> Currently a personal/early-stage project. v0.2.6 is the first
> release that ships cross-device sync.
