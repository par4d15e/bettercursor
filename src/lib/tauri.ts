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
