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
