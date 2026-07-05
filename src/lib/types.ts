// src/lib/types.ts — TS mirror of Rust core::canonical types

export type SourceLayer = "mac" | "linux_cli" | "linux_desktop";

export interface SourceInfo {
  last_seen_at: number;
  layer: string;
  path: string;
}

export interface Sources {
  mac?: SourceInfo;
  linux_cli?: SourceInfo;
  linux_desktop?: SourceInfo;
}

export interface CanonicalSession {
  uuid: string;
  project_slug: string;
  project_path: string;
  chat_root: string;
  name: string;
  last_updated_at: number;
  bubble_count: number;
  is_empty_draft: boolean;
  /** True when a data-correctness issue was detected (e.g. broken Layer 2 root blob). */
  is_broken: boolean;
  /** Human-readable explanation of `is_broken`. Present iff `is_broken == true`. */
  broken_reason?: string;
  sources: Sources;
  first_user_message_preview: string;
  files_referenced: string[];
  /** Concatenated conversation text (≤2 KB), used for full-content search. */
  indexable_text: string;
  /** True if Layer 3 (state.vscdb composerData) has a corresponding
   *  entry for this uuid. False = CLI-originated, Desktop Sidebar
   *  can't see it; the inject-to-Layer-3 button shows in that case. */
  layer_3_present: boolean;
  /** v0.3.4: L3 row exists but bubble content is stale — show re-sync. */
  layer_3_needs_refresh?: boolean;
  /** v0.3.4: L2 store.db exists but DAG/content is stale vs L3 — show re-sync. */
  layer_2_needs_refresh?: boolean;
  /** v0.3.5: immutable creation endpoint (from unified.db session_origins). */
  created_endpoint?: SourceLayer | null;
  /** Epoch ms when the session was first created on `created_endpoint`. */
  created_at_ms?: number | null;
  /** True when Layer 2 meta[0].subagentInfo is present (Task subagent). */
  is_subagent?: boolean;
  /** Parsed subagentInfo from Layer 2 when `is_subagent`. */
  subagent_info?: SubagentInfo | null;
}

export interface SubagentInfo {
  parent_agent_id: string;
  root_parent_agent_id: string;
  tool_call_id?: string | null;
  type_name?: string | null;
}

// v0.2.5: SOURCE_LABELS moved to `useTranslation()` in SourceBadge
// (namespace `source.*` in src/locales). Kept the colors map here
// since Tailwind class strings are platform-agnostic.

export const SOURCE_COLORS: Record<SourceLayer, string> = {
  mac: "bg-accent-blue/20 text-accent-blue border-accent-blue/30",
  linux_cli: "bg-accent-green/20 text-accent-green border-accent-green/30",
  linux_desktop: "bg-accent-purple/20 text-accent-purple border-accent-purple/30",
};
