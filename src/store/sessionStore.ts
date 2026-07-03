// src/store/sessionStore.ts — Zustand store

import { create } from "zustand";
import { useMemo } from "react";
import type { CanonicalSession } from "../lib/types";
import { listSessions, refreshSessions, onSessionsUpdated } from "../lib/tauri";

/**
 * Sort order. Drives the project-group → session ordering inside each
 * group, plus the cursor across groups (groups themselves stay alphabetical
 * by slug regardless of mode).
 */
export type SortMode = "updated_desc" | "name_asc" | "bubble_count_desc";

export const SORT_MODES: SortMode[] = [
  "updated_desc",
  "name_asc",
  "bubble_count_desc",
];

export const SORT_LABELS: Record<SortMode, string> = {
  updated_desc: "最近更新 ↓",
  name_asc: "名称 A→Z",
  bubble_count_desc: "气泡数 ↓",
};

interface SessionState {
  sessions: CanonicalSession[];
  selectedUuid: string | null;
  loading: boolean;
  error: string | null;
  lastScanAt: string | null;
  search: string;
  sortMode: SortMode;
  setSessions: (s: CanonicalSession[]) => void;
  setSelected: (uuid: string | null) => void;
  setSearch: (q: string) => void;
  setSortMode: (m: SortMode) => void;
  cycleSortMode: () => void;
  refresh: () => Promise<void>;
  init: () => Promise<void>;
}

export const useSessionStore = create<SessionState>((set) => ({
  sessions: [],
  selectedUuid: null,
  loading: false,
  error: null,
  lastScanAt: null,
  search: "",
  sortMode: "updated_desc",

  setSessions: (sessions) => set({ sessions }),
  setSelected: (uuid) => set({ selectedUuid: uuid }),
  setSearch: (search) => set({ search }),
  setSortMode: (sortMode) => set({ sortMode }),
  cycleSortMode: () =>
    set((s) => {
      const i = SORT_MODES.indexOf(s.sortMode);
      const next = SORT_MODES[(i + 1) % SORT_MODES.length];
      return { sortMode: next };
    }),

  refresh: async () => {
    set({ loading: true, error: null });
    try {
      console.log("[bettercursor] refresh: calling Tauri command...");
      const count = await refreshSessions();
      console.log(`[bettercursor] refresh: Rust returned ${count} sessions`);
      const fresh = await listSessions();
      console.log(`[bettercursor] refresh: listSessions returned ${fresh.length} sessions`);
      set({
        sessions: fresh,
        lastScanAt: new Date().toISOString(),
        loading: false,
      });
    } catch (e: any) {
      console.error(`[bettercursor] refresh failed:`, e);
      set({ error: String(e), loading: false });
    }
  },

  init: async () => {
    console.log("[bettercursor] init: starting");
    // Subscribe to backend events (emitted on initial scan + manual refresh).
    await onSessionsUpdated(async (count) => {
      console.log(`[bettercursor] event: sessions-updated, count=${count}`);
      const fresh = await listSessions();
      set({ sessions: fresh, lastScanAt: new Date().toISOString() });
      console.log(`[bettercursor] state updated, sessions=${fresh.length}`);
    });
    // Pull initial data.
    try {
      const fresh = await listSessions();
      console.log(`[bettercursor] initial listSessions: ${fresh.length} sessions`);
      set({ sessions: fresh, lastScanAt: new Date().toISOString() });
    } catch (e: any) {
      console.error(`[bettercursor] init failed:`, e);
      set({ error: String(e) });
    }
  },
}));

// ── Derived selectors (pure functions, safe to memoize) ───────
// IMPORTANT: Selectors that build new objects each call must NEVER feed
// directly into `useSessionStore(selector)` without memoization — Zustand
// uses `Object.is` per `useSyncExternalStore`, and any new reference
// triggers an infinite re-render loop that React 19 bails out with
// "Maximum update depth exceeded".
//
// Pattern: subscribe to the underlying ARRAY/STRING refs (cheap to
// compare), then derive in `useMemo` with those as dependencies.

export function filterSessions(
  sessions: CanonicalSession[],
  query: string,
): CanonicalSession[] {
  const q = query.trim().toLowerCase();
  if (!q) return sessions;
  return sessions.filter(
    (sess) =>
      sess.name.toLowerCase().includes(q) ||
      sess.project_slug.toLowerCase().includes(q) ||
      sess.first_user_message_preview.toLowerCase().includes(q) ||
      sess.indexable_text.toLowerCase().includes(q) ||
      sess.uuid.toLowerCase().includes(q),
  );
}

export interface ProjectGroup {
  slug: string;
  sessions: CanonicalSession[];
}

function compareSessions(a: CanonicalSession, b: CanonicalSession, mode: SortMode): number {
  switch (mode) {
    case "updated_desc":
      return b.last_updated_at - a.last_updated_at;
    case "name_asc":
      return a.name.localeCompare(b.name, "zh-Hans-CN");
    case "bubble_count_desc":
      return b.bubble_count - a.bubble_count;
  }
}

export function groupSessionsByProject(
  sessions: CanonicalSession[],
  sortMode: SortMode = "updated_desc",
): ProjectGroup[] {
  const groups = new Map<string, CanonicalSession[]>();
  for (const sess of sessions) {
    const arr = groups.get(sess.project_slug) ?? [];
    arr.push(sess);
    groups.set(sess.project_slug, arr);
  }
  return Array.from(groups.entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([slug, list]) => ({
      slug,
      sessions: [...list].sort((a, b) => compareSessions(a, b, sortMode)),
    }));
}

// Hook: filtered + grouped sessions, fully memoized via React.useMemo.
// Subscribes to the raw array + search string + sortMode refs, then
// derives once per real change — not on every render.
export function useGroupedSessions(): ProjectGroup[] {
  const sessions = useSessionStore((s) => s.sessions);
  const search = useSessionStore((s) => s.search);
  const sortMode = useSessionStore((s) => s.sortMode);
  return useMemo(
    () => groupSessionsByProject(filterSessions(sessions, search), sortMode),
    [sessions, search, sortMode],
  );
}
