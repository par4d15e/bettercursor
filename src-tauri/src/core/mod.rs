//! bettercursor core — pure Rust, no Tauri dependencies.
//!
//! Submodules:
//!   - paths      — 4-layer storage path resolution
//!   - storage    — WAL-safe SQLite read
//!   - canonical  — merge sessions across layers
//!   - watcher    — fs watcher for auto-sync (notify + poll fallback)
//!   - config     — user preferences (~/.bettercursor/config.json)
//!   - inject     — Layer 3 entry synthesis (CLI session → Desktop Sidebar)
//!   - sync       — v0.2-alpha one-click L2↔L3 补层 sync
//!   - process    — Cursor / cursor-agent process detection (sync safety check)
//!   - transport  — v0.2.6 cross-device sync: Transport trait + SSH/rsync impl
//!                  + SessionSnapshot codec + ~/.bettercursor/transports.json
//!   - unified    — v0.3.0 ~/.bettercursor/unified.db (per SYNC_DESIGN §3):
//!                  7 tables + FTS5 + rebuild_from_cursor_state + archive +
//!                  conflicts + sync_runs

pub mod canonical;
pub mod config;
pub mod conflict;
pub mod inject;
pub mod paths;
pub mod process;
pub mod snapshot;
pub mod storage;
pub mod sync;
pub mod transport;
pub mod unified;
pub mod watcher;
