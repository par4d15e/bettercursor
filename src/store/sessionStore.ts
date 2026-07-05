// src/store/sessionStore.ts — Zustand store

import { create } from "zustand";
import { useMemo } from "react";
import type { CanonicalSession } from "../lib/types";
import {
  listSessions,
  syncNow,
  onSessionsUpdated,
  watcherStatus,
} from "../lib/tauri";

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

// v0.2.5: i18n-aware. Pass the `t` from `useTranslation()` so the
// toolbar label follows the active locale. Used by SessionTree.tsx.
export function getSortLabels(
  t: (key: string) => string,
): Record<SortMode, string> {
  return {
    updated_desc: t("tree.sort.updated_desc"),
    name_asc: t("tree.sort.name_asc"),
    bubble_count_desc: t("tree.sort.bubble_count_desc"),
  };
}

interface SessionState {
  sessions: CanonicalSession[];
  selectedUuid: string | null;
  loading: boolean;
  error: string | null;
  lastScanAt: string | null;
  search: string;
  sortMode: SortMode;
  /// True iff the backend fs-watcher thread is alive. Drives a "live"
  /// badge in the toolbar so users see auto-sync is working. The watcher
  /// always runs in v0.2-alpha (no toggle — #103); this just exposes
  /// thread state for diagnostics.
  autoSyncLive: boolean;
  watcherDirs: string[];
  /// v0.2.3: epoch ms of the last successful scan, sourced from the
  /// Rust `watcher_status.last_scan_at_ms` field. The SyncStatusBadge
  /// formats this as "12s 前" / "3m 前" via its `formatAge` helper.
  /// `null` before the first scan completes.
  last_scan_at_ms: number | null;
  setSessions: (s: CanonicalSession[]) => void;
  setSelected: (uuid: string | null) => void;
  setSearch: (q: string) => void;
  setSortMode: (m: SortMode) => void;
  cycleSortMode: () => void;
  /// v0.2.3 rename: was `refresh()`. Now `syncNow()` matches the Rust
  /// `sync_now` command and the PRD / SYNC_DESIGN v0.2+ wording. Same
  /// semantics — full local Cursor re-scan + refresh cache.
  syncNow: () => Promise<void>;
  init: () => Promise<void>;
  refreshWatcherStatus: () => Promise<void>;
  /// v0.2.3: kick off a 5-second polling loop that re-fetches
  /// `watcher_status` so the SyncStatusBadge stays in sync with the
  /// backend's `AppState.last_scan_at`. Returns an unsubscribe fn —
  /// the caller must call it on unmount to clear the interval.
  startWatcherPolling: () => () => void;
}

export const useSessionStore = create<SessionState>((set) => ({
  sessions: [],
  selectedUuid: null,
  loading: false,
  error: null,
  lastScanAt: null,
  search: "",
  sortMode: "updated_desc",
  autoSyncLive: false,
  watcherDirs: [],
  last_scan_at_ms: null,

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

  syncNow: async () => {
    set({ loading: true, error: null });
    try {
      console.log("[bettercursor] syncNow: calling Tauri command...");
      const count = await syncNow();
      console.log(`[bettercursor] syncNow: Rust returned ${count} sessions`);
      const fresh = await listSessions();
      console.log(
        `[bettercursor] syncNow: listSessions returned ${fresh.length} sessions`,
      );
      // Re-pull watcher status so the SyncStatusBadge updates its
      // "Xs 前" counter to ~0 right after the manual scan.
      let last_scan_at_ms: number | null = null;
      let autoSyncLive = false;
      let watcherDirs: string[] = [];
      try {
        const w = await watcherStatus();
        last_scan_at_ms = w.last_scan_at_ms;
        autoSyncLive = w.active;
        watcherDirs = w.dirs;
      } catch (e) {
        console.warn("[bettercursor] syncNow: watcher_status poll failed:", e);
      }
      set({
        sessions: fresh,
        lastScanAt: new Date().toISOString(),
        loading: false,
        last_scan_at_ms,
        autoSyncLive,
        watcherDirs,
      });
    } catch (e: any) {
      console.error(`[bettercursor] syncNow failed:`, e);
      set({ error: String(e), loading: false });
    }
  },

  init: async () => {
    console.log("[bettercursor] init: starting");
    // Subscribe to backend events (emitted on initial scan + manual
    // sync_now + watcher auto-sync).
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
    // Probe watcher status (non-fatal if it errors).
    try {
      const w = await watcherStatus();
      set({
        autoSyncLive: w.active,
        watcherDirs: w.dirs,
        last_scan_at_ms: w.last_scan_at_ms,
      });
      console.log(
        `[bettercursor] watcher: active=${w.active}, last_scan_at_ms=${w.last_scan_at_ms}, dirs=${w.dirs.length}`,
      );
    } catch (e: any) {
      console.warn(`[bettercursor] watcher_status failed:`, e);
    }
  },

  refreshWatcherStatus: async () => {
    try {
      const w = await watcherStatus();
      set({
        autoSyncLive: w.active,
        watcherDirs: w.dirs,
        last_scan_at_ms: w.last_scan_at_ms,
      });
    } catch (e: any) {
      console.warn("[bettercursor] refreshWatcherStatus:", e);
    }
  },

  startWatcherPolling: () => {
    // Poll every 5s — watcher scan interval is 30s, so 5s gives us
    // a smooth "Xs 前" counter without hammering Tauri IPC.
    const id = setInterval(() => {
      // Reuse refreshWatcherStatus via getState() to avoid capturing
      // a stale `set` closure. Safe because Zustand actions are stable
      // across the lifetime of the store.
      useSessionStore.getState().refreshWatcherStatus();
    }, 5000);
    return () => clearInterval(id);
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
  sessions: SessionTreeNode[];
}

export interface SessionTreeNode {
  session: CanonicalSession;
  children: SessionTreeNode[];
}

function resolveSubagentParentId(
  sess: CanonicalSession,
  idsInGroup: Set<string>,
): string | null {
  if (!sess.is_subagent || !sess.subagent_info) return null;
  // Always hang under the root conversation — parentAgentId may be another
  // subagent (Task chain), not the user-visible parent session.
  const root = sess.subagent_info.root_parent_agent_id;
  if (idsInGroup.has(root) && root !== sess.uuid) {
    return root;
  }
  return null;
}

/** Nest subagent sessions under their parent within one project bucket. */
export function buildSessionTree(
  sessions: CanonicalSession[],
  sortMode: SortMode = "updated_desc",
): SessionTreeNode[] {
  if (sessions.length === 0) return [];

  const ids = new Set(sessions.map((s) => s.uuid));
  const nodes = new Map<string, SessionTreeNode>();
  for (const s of sessions) {
    nodes.set(s.uuid, { session: s, children: [] });
  }

  const roots: SessionTreeNode[] = [];
  for (const s of sessions) {
    const node = nodes.get(s.uuid)!;
    const parentId = resolveSubagentParentId(s, ids);
    if (parentId) {
      nodes.get(parentId)?.children.push(node);
    } else {
      roots.push(node);
    }
  }

  const sortNodes = (list: SessionTreeNode[]): SessionTreeNode[] =>
    [...list]
      .sort((a, b) => compareSessions(a.session, b.session, sortMode))
      .map((n) => ({
        session: n.session,
        children: sortNodes(n.children),
      }));

  return sortNodes(roots);
}

export function countSessionTreeNodes(nodes: SessionTreeNode[]): number {
  return nodes.reduce(
    (sum, n) => sum + 1 + countSessionTreeNodes(n.children),
    0,
  );
}

/** Parent UUIDs on the path to `uuid` (nearest parent first). */
export function ancestorSessionIds(
  groups: ProjectGroup[],
  uuid: string,
): string[] {
  const walk = (
    nodes: SessionTreeNode[],
    trail: string[],
  ): string[] | null => {
    for (const n of nodes) {
      if (n.session.uuid === uuid) return trail;
      const found = walk(n.children, [...trail, n.session.uuid]);
      if (found) return found;
    }
    return null;
  };
  for (const g of groups) {
    const found = walk(g.sessions, []);
    if (found) return found;
  }
  return [];
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
      sessions: buildSessionTree(list, sortMode),
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