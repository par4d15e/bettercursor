# bettercursor

> Local **Cursor** session viewer (read-only). **Tauri 2 + React 19 + Rust**, UI inspired by [cc-switch](https://github.com/farion1231/cc-switch).
>
> 🌐 [English](README.md) · [简体中文](README.zh-CN.md)

![status](https://img.shields.io/badge/status-v0.2.5-success)
![platform](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue)
![stack](https://img.shields.io/badge/Tauri-2-orange)
![language](https://img.shields.io/badge/Rust-1.77%2B-orange)
![i18n](https://img.shields.io/badge/i18n-zh--CN%20%7C%20en-green)

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

### v0.2.5 (✅ current, shipped 2026-07-04)

- [x] **Cross-platform packaging**: Linux `.deb` / `.AppImage` + macOS
      unsigned `.dmg` (macOS 10.15+, Apple Silicon only for now) + Windows
      `.msi` / `.exe` (NSIS). All built by GitHub Actions matrix
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

### v0.2.6 / v0.3 (planned, see [SYNC_DESIGN.md](SYNC_DESIGN.md))

- [ ] Cross-device sync (Tailscale / SSH-rsync) — §4 `Transport` trait
      first cut
- [ ] `~/.bettercursor/unified.db` (snapshot codec + `Conflict` enum
      — major version)
- [ ] Outbox flush + 5-way conflict categorization UI
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
sudo dpkg -i BetterCursor_0.2.5_amd64.deb
sudo apt-get install -f   # satisfy missing deps if dpkg complains

# Portable AppImage (no install, but first run downloads linuxdeploy
# binaries from tauri-apps/binary-releases — needs network)
chmod +x BetterCursor_0.2.5_amd64.AppImage
./BetterCursor_0.2.5_amd64.AppImage
```

### macOS

1. Download `BetterCursor_0.2.5_aarch64.dmg` (Apple Silicon).
   Intel `.dmg` is pending a release.yml tweak (matrix split) — see
   `task #37` / v0.2.6 milestone.
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
msiexec /i BetterCursor_0.2.5_x64_en-US.msi

# or .exe (NSIS — better for personal installs)
.\BetterCursor_0.2.5_x64-setup.exe
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
├── PRD.md · SYNC_DESIGN.md · TAURI_RUST_PLAN.md · BACKGROUND.md · goal.md
└── .github/workflows/     # release.yml (3-OS matrix)
```

## Architecture overview

Session reads happen in **three layers**:

| Layer | Storage | Path (Linux) | Role |
|---|---|---|---|
| **L1** | JSONL | `<workspaceStorage>/<chat_root>/<composer>/<session>.jsonl` | Latest, primary Cursor CLI write target |
| **L2** | SQLite `ItemTable` | `<…>/state.vscdb` (aiDiskKV) | In-editor conversation cache |
| **L3** | SQLite `cursorDiskKV` | `<…>/state.vscdb` (cursorDiskKV) | In-editor metadata |

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
| [TAURI_RUST_PLAN.md](TAURI_RUST_PLAN.md) | Python → Rust module mapping + Cargo dep manifest |
| [BACKGROUND.md](BACKGROUND.md) | Project history (Python daemon → Tauri rewrite) |
| [goal.md](goal.md) | Original brief |

## Roadmap

```
v0.2.5 (✅ now)  Cross-platform packaging · i18n · background sync ·
                conversation records · repair orphan · delete
v0.2.6 (next)   Cross-device sync (Transport trait first cut)
v0.3.0 (later)  ~/.bettercursor/unified.db · snapshot codec ·
                Conflict UI
```

## Acknowledgements

- UI paradigm: [farion1231/cc-switch](https://github.com/farion1231/cc-switch)
- Old Python daemon (`bettercursor/`, `adapter/`) provided the parsing
  algorithm reference
- `vendored/cursaves/` is a snapshot of the upstream Cursor parsing
  library

---

> Currently a personal/early-stage project. v0.2.5 is the first
> packaged release available for public download.
