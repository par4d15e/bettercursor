// src/lib/tauri.ts — typed wrappers around `invoke()`

import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import type { CanonicalSession } from "./types";

export async function listSessions(): Promise<CanonicalSession[]> {
  return invoke<CanonicalSession[]>("list_sessions");
}

export async function refreshSessions(): Promise<number> {
  return invoke<number>("refresh_sessions");
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
  role: string; // "user" | "assistant"
  text: string;
  tool_calls: BubbleToolUse[];
  files: string[];
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

export interface WatcherStatus {
  active: boolean;
  /// User opt-in for the auto-sync behavior (ccswitch-style toggle).
  /// When `false`, the watcher thread is still alive but skips scans.
  enabled: boolean;
  dirs: string[];
}

export async function watcherStatus(): Promise<WatcherStatus> {
  return invoke<WatcherStatus>("watcher_status");
}

/// Toggle the auto-sync preference. Persists to
/// `~/.bettercursor/config.json`. Returns the fresh status after the
/// toggle completes so the UI badge can refresh in one round-trip.
export async function setAutoSync(enabled: boolean): Promise<WatcherStatus> {
  return invoke<WatcherStatus>("set_auto_sync", { enabled });
}

// ── Layer 3 injection (CLI session → Desktop Sidebar) ─────────

export interface InjectSources {
  layer1_jsonl: string | null;
  layer2_store_db: string | null;
  layer2_meta_json: string | null;
  cwd: string | null;
  title: string | null;
  created_at_ms: number | null;
}

export interface InjectMutation {
  op: "item_table_upsert" | "disk_kv_upsert";
  key: string;
  value_hex: string;
}

export interface InjectPlan {
  uuid: string;
  mutations: InjectMutation[];
  sources: InjectSources;
  /// When non-null, the injector skipped building mutations and the
  /// UI should surface this message instead of a "Confirm" button.
  skip_reason: string | null;
}

/** Result of staging a Layer 3 injection to disk. The user must
 *  quit Cursor Electron and then run `apply_command` themselves —
 *  bettercursor never touches state.vscdb while Cursor holds it
 *  open (see #84). */
export interface PrepareResult {
  uuid: string;
  /** Absolute path to `~/.bettercursor/queue/inject-<uuid>.json`. */
  queue_path: string;
  /** One-liner the user copies-and-pastes after closing Cursor.
   *  Format: `python3 ~/.bettercursor/apply.py <queue_path>`. */
  apply_command: string;
  /** Count of mutations the apply script will run, useful for the
   *  "准备离线注入包 → N 条变更即将落地" preview. */
  mutations: number;
}

/** Returned by `inspectPreparedLayer3`: tells the UI whether the
 *  queue file exists and whether apply.py has already finished for
 *  this uuid (detected by sidecar `.applied` marker). */
export interface Prepared {
  uuid: string;
  queue_path: string;
  applied: boolean;
  marker_path: string;
  apply_command: string;
}

/// Build a previewable plan for synthesizing the Layer 3 entries
/// that would make this CLI-originated session visible in Cursor
/// Electron Desktop's Sidebar. Pure read; no disk writes.
export async function dryRunInjectLayer3(uuid: string): Promise<InjectPlan> {
  return invoke<InjectPlan>("dry_run_inject_layer3", { uuid });
}

/// Stage a Layer 3 injection: writes the plan envelope to
/// `~/.bettercursor/queue/inject-<uuid>.json`. Idempotent — calling
/// again overwrites with a fresh plan. Refuses to queue if the
/// plan carries a `skip_reason` (no source data).
///
/// The returned `apply_command` must be run manually by the user
/// after quitting Cursor Electron. bettercursor does NOT run it
/// for them — see #84 for why live writes lost data.
export async function prepareInjectLayer3(uuid: string): Promise<PrepareResult> {
  return invoke<PrepareResult>("prepare_inject_layer3", { uuid });
}

/// Inspect a previously staged injection. Returns `null` when no
/// queue file exists yet (use this to decide whether to show the
/// "已应用" badge or the "准备离线注入包" call-to-action).
export async function inspectPreparedLayer3(uuid: string): Promise<Prepared | null> {
  return invoke<Prepared | null>("inspect_prepared_layer3", { uuid });
}
