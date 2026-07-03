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
  sources: Sources;
  first_user_message_preview: string;
  files_referenced: string[];
}

export const SOURCE_LABELS: Record<SourceLayer, string> = {
  mac: "Mac Desktop",
  linux_cli: "Linux CLI",
  linux_desktop: "Linux Desktop",
};

export const SOURCE_COLORS: Record<SourceLayer, string> = {
  mac: "bg-accent-blue/20 text-accent-blue border-accent-blue/30",
  linux_cli: "bg-accent-green/20 text-accent-green border-accent-green/30",
  linux_desktop: "bg-accent-purple/20 text-accent-purple border-accent-purple/30",
};
