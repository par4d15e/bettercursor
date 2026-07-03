//! bettercursor core — pure Rust, no Tauri dependencies.
//!
//! Submodules:
//!   - paths      — 4-layer storage path resolution
//!   - storage    — WAL-safe SQLite read
//!   - canonical  — merge sessions across layers
//!   - watcher    — fs watcher for auto-sync (notify + poll fallback)

pub mod canonical;
pub mod paths;
pub mod storage;
pub mod watcher;
