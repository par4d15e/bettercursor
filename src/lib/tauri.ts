// src/lib/tauri.ts — typed wrappers around `invoke()`

import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import type { CanonicalSession } from "./types";

export async function listSessions(): Promise<CanonicalSession[]> {
  return invoke<CanonicalSession[]>("list_sessions");
}

/// v0.2.3 rename: was `refreshSessions` (v0.1 terminology). Now
/// `syncNow` matches the Rust command `sync_now` and the PRD /
/// SYNC_DESIGN v0.2+ wording. Same semantics: full local Cursor
/// re-scan, cache refresh, emit `sessions-updated`.
export async function syncNow(): Promise<number> {
  return invoke<number>("sync_now");
}

export async function getResumeCommand(
  uuid: string,
  source: string,
): Promise<string> {
  return invoke<string>("get_resume_command", { uuid, source });
}

export async function platformInfo(): Promise<string> {
  return invoke<string>("platform_info");
}

export function onSessionsUpdated(
  cb: (count: number) => void,
): Promise<UnlistenFn> {
  return listen<number>("sessions-updated", (e) => cb(e.payload));
}

// ── Conversation / bubbles (Layer 1 JSONL) ────────────────────

export interface BubbleToolUse {
  name: string;
  input?: unknown;
}

export interface Bubble {
  /// v0.2.2: stable 36-char GUID. Filled by the Rust side via
  /// `inject::deterministic_bubble_id` for L1 bubbles; L3 bubbles use
  /// the `<bid>` portion of `bubbleId:<uuid>:<bid>`; L2 blobs without
  /// an explicit id get a deterministic synthesized one. Empty string
  /// is treated as "no id" — `MessageList` falls back to `idx-${i}` so
  /// React still has a unique key.
  id?: string;
  role: string; // "user" | "assistant"
  text: string;
  tool_calls: BubbleToolUse[];
  files: string[];
  /// v0.2.2: epoch ms. L1 JSONL often has no reliable timestamp and
  /// returns 0; L3 rows and L2 blobs have it when present. Optional
  /// here (defaults to 0) so old hardcoded fixtures keep type-checking.
  created_at_ms?: number;
}

export interface Conversation {
  uuid: string;
  bubbles: Bubble[];
  source_path: string | null;
  total_lines: number;
  parse_errors: number;
}

export async function getConversation(uuid: string): Promise<Conversation> {
  return invoke<Conversation>("get_conversation", { uuid });
}

// ── Live fs watcher (auto-sync) ────────────────────────────────

/// Diagnostics for the always-on fs watcher (v0.2-alpha removed the
/// user toggle — see #103). The watcher thread always runs and always
/// re-scans on fs events. This struct is kept only for the "live" /
/// "stopped" badge in the sidebar header.
///
/// v0.2.3: `last_scan_at_ms` added so the SyncStatusBadge can render
/// a "12s 前" / "3m 前" counter without re-running a Tauri command
/// every tick.
export interface WatcherStatus {
  active: boolean;
  dirs: string[];
  /// Epoch ms of the last successful scan (fs event or polling
  /// fallback). `null` before the first scan completes. Format with
  /// `<SyncStatusBadge>`'s `formatAge` helper.
  last_scan_at_ms: number | null;
}

export async function watcherStatus(): Promise<WatcherStatus> {
  return invoke<WatcherStatus>("watcher_status");
}

// ── v0.2-alpha 一键 L2↔L3 补层同步 ────────────────────────

/// Result of one manual sync. Mirrors `core::sync::SyncReport` in
/// Rust. `wrote_layer2` / `wrote_layer3` indicate which missing
/// layers were synthesized on disk. `skipped` is the soft-skip list
/// (e.g. "cursor_running", "already_synced") — never empty for a
/// successful call where no writes happened.
export interface SyncReport {
  uuid: string;
  wrote_layer2: boolean;
  wrote_layer3: boolean;
  skipped: string[];
  /** Hex SHA256 of the root blob, when Layer 2 was written. */
  root_blob_id: string | null;
  duration_ms: number;
}

/// Run the manual L2↔L3 补层 sync for one session. `cwd` is supplied
/// from the session's `project_path` (sourced from L3's
/// `workspaceIdentifier.uri.fsPath` when available). The Rust side
/// refuses to proceed if Cursor or `cursor-agent` is running — the
/// caller should surface that error verbatim in the UI.
export async function syncSessionLayer23(
  uuid: string,
  cwd: string | null,
): Promise<SyncReport> {
  return invoke<SyncReport>("sync_session_layer23", { uuid, cwd });
}

// ── v0.2.1 修复 orphan + 删除 session ──────────────────────

/// Result of one bulk `fix_orphans` run. Mirrors
/// `core::sync::FixOrphansReport` in Rust. `fixed` lists the uuids that
/// got their `latestRootBlobId` filled in (and a `.backup_<ts>` left on
/// disk). `skipped` lists uuids the repair failed for (per-session
/// error message). `scanned` is the total session count walked across
/// all `~/.cursor/chats/*/store.db` files.
export interface FixOrphansReport {
  fixed: string[];
  skipped: string[];
  scanned: number;
}

/// Result of a `delete_session` call. Mirrors `core::sync::DeleteReport`.
/// Each layer gets an independent removal flag + an optional skip
/// reason (`"l1_not_present"` / `"l2_not_present"` / `"slug_not_provided"`
/// / `"invalid_slug"` / `"io_error: ..."`). When `cursorRunning` is
/// `true` the call short-circuited — both removed_* flags are `false`,
/// both skipped_* are `null`, and `runningProcesses` lists the pids
/// that triggered the guard.
export interface DeleteReport {
  uuid: string;
  removed_l1: boolean;
  removed_l2: boolean;
  skipped_l1: string | null;
  skipped_l2: string | null;
  cursor_running: boolean;
  running_processes: string[];
}

/// Walk every `~/.cursor/chats/*/<uuid>/store.db` and repair sessions
/// whose `meta[0].latestRootBlobId` is an empty string. Each repaired
/// store.db gets a `.backup_<ts>` sibling left on disk — the call is
/// non-destructive. Returns the per-session outcome list.
export async function fixOrphans(): Promise<FixOrphansReport> {
  return invoke<FixOrphansReport>("fix_orphans");
}

/// Delete one session's Layer 1 (JSONL) + Layer 2 (store.db) from
/// disk. Layer 3 (state.vscdb composerData) is intentionally skipped
/// by the backend — Cursor Desktop owns that storage. `cwd` powers the
/// L2 path (`md5(cwd)` is the bucket name under `~/.cursor/chats/`).
/// `projectSlug` (from `CanonicalSession.project_slug`) is needed for
/// the L1 path under `~/.cursor/projects/<slug>/agent-transcripts/`;
/// pass `null` to skip L1. The call refuses to mutate anything while
/// Cursor / cursor-agent is running — check `cursor_running` on the
/// returned report.
export async function deleteSession(
  uuid: string,
  cwd: string | null,
  projectSlug: string | null,
): Promise<DeleteReport> {
  return invoke<DeleteReport>("delete_session", {
    uuid,
    cwd,
    projectSlug,
  });
}

// ── v0.2.6 cross-device sync (Transport trait + SSH/rsync) ────
//
// Four Tauri commands wrap the Rust `Transport` trait:
//   - list peers from ~/.bettercursor/transports.json
//   - probe a peer's SSH connectivity (returns latency + error)
//   - push one session's metadata to a peer
//   - pull metadata from a peer (since optional epoch ms)
//
// v0.2.6 first cut: SSH+rsync only (T2 per SYNC_DESIGN §4.3). v0.3.0+
// will add git (T3), S3 (T4), Tailscale (T5), folder watcher (T1), and
/// a UI (`<SyncPeersDialog>`).

export interface PeerSummary {
  id: string;
  kind: string;
  host: string;
  port: number;
  identity_file: string;
  remote_snap_dir: string;
  remote_hostname: string;
}

export interface TestReport {
  peer_id: string;
  ok: boolean;
  latency_ms: number;
  error?: string;
}

export interface PushReport {
  uuid: string;
  bytes_written: number;
  duration_ms: number;
}

export interface RemoteSession {
  uuid: string;
  host: string;
  last_updated_at_ms: number;
  project_slug: string;
  source_path: string;
}

export interface PullReport {
  peer_id: string;
  count: number;
  snapshots: RemoteSession[];
}

/// List every peer declared in `~/.bettercursor/transports.json`.
/// Empty array when the file doesn't exist yet (first run).
export async function transportListPeers(): Promise<PeerSummary[]> {
  return invoke<PeerSummary[]>("transport_list_peers");
}

/// Probe one peer's SSH connectivity by running `ssh <peer> true` on
/// the backend. Returns a `TestReport` (never throws) — when `ok` is
/// `false`, the human-readable reason is in `error`.
export async function transportTest(peerId: string): Promise<TestReport> {
  return invoke<TestReport>("transport_test", { peerId });
}

/// Push one session's metadata snapshot to a peer. Resolves to a
/// `PushReport` on success; rejects with the ssh/rsync stderr on
/// failure. The session must be present in the current local scan —
/// pass a uuid you got from `listSessions()`.
export async function transportPush(
  uuid: string,
  peerId: string,
): Promise<PushReport> {
  return invoke<PushReport>("transport_push", { uuid, peerId });
}

/// Pull session metadata from a peer. `sinceMs` defaults to `0`
/// (everything). v0.2.6 doesn't write a local DB — the returned
/// snapshots are surfaced for inspection only.
export async function transportPull(
  peerId: string,
  sinceMs?: number,
): Promise<PullReport> {
  return invoke<PullReport>("transport_pull", { peerId, sinceMs });
}
