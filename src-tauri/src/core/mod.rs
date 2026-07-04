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

pub mod canonical;
pub mod config;
pub mod inject;
pub mod paths;
pub mod process;
pub mod storage;
pub mod sync;
pub mod transport;
pub mod watcher;
