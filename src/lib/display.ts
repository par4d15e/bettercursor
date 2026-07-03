// src/lib/display.ts — display-only derivation utilities.
//
// Cursor's storage doesn't always populate a human-readable `name` for
// sessions (depends on whether the user named it, and which layer it
// came from). To make the list useful, we derive a `resolveTitle()`
// from what's available, in this priority:
//
//   1. `name`                           (user-given or layer-specific)
//   2. `first_user_message_preview`     (Layer 1 JSONL first user text,
//                                        first line, truncated to 60 chars)
//   3. `<untitled> · <first 8 of uuid>` (always non-empty fallback, marked
//                                        so callers can render it muted)
//
// The structured `DisplayTitle` carries an `isFallback` flag so the UI
// can render the fallback case with a different visual style (italic +
// muted color), making it obvious to users that the row had no real
// title extracted from the source data.
//
// This intentionally stays in the TS layer so we don't bump the
// Rust→TS schema contract for what is purely a UI concern.

import type { CanonicalSession } from "./types";

export const TITLE_PREVIEW_MAX = 60;

export interface DisplayTitle {
  text: string;
  isFallback: boolean;
}

export function resolveTitle(s: CanonicalSession): DisplayTitle {
  const name = s.name?.trim();
  if (name && name !== "New Agent") {
    return { text: name, isFallback: false };
  }

  const preview = s.first_user_message_preview
    ?.split("\n", 1)[0]
    ?.trim();
  if (preview) {
    return {
      text:
        preview.length > TITLE_PREVIEW_MAX
          ? preview.slice(0, TITLE_PREVIEW_MAX) + "…"
          : preview,
      isFallback: false,
    };
  }

  return {
    text: `Untitled · ${s.uuid.slice(0, 8)}`,
    isFallback: true,
  };
}

// Convenience: just the string (for callers that don't care about styling).
export function displayTitle(s: CanonicalSession): string {
  return resolveTitle(s).text;
}
