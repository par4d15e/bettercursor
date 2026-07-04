// src/components/SyncStatusBadge.tsx — app header status indicator.
//
// v0.2.3: replaces the old "LIVE" badge in SessionTree. Subscribes to
// the store's `last_scan_at_ms` + `autoSyncLive` fields, ticks every
// second to render a fresh "Xs 前" / "Xm 前" / "Xh 前" counter, and
// polls the backend's `watcher_status` command every 5s so the
// counter stays in sync with `AppState.last_scan_at` (which the
// watcher thread bumps on every fs event / 30s polling tick).
//
// `formatAge` is exported (alongside its tests in
// SyncStatusBadge.test.tsx) so other components can reuse the same
// formatter without duplicating logic.

import { useEffect, useState } from "react";
import { useSessionStore } from "../store/sessionStore";

export function SyncStatusBadge() {
  const autoSyncLive = useSessionStore((s) => s.autoSyncLive);
  const watcherDirs = useSessionStore((s) => s.watcherDirs);
  const last_scan_at_ms = useSessionStore((s) => s.last_scan_at_ms);
  const startWatcherPolling = useSessionStore((s) => s.startWatcherPolling);
  const init = useSessionStore((s) => s.init);

  // 1Hz local tick so the "Xs 前" counter updates without waiting for
  // the next 5s poll. Cheap (single setState per second) and bounded
  // to this component, so it doesn't cascade into other re-renders.
  const [now, setNow] = useState(() => Date.now());

  // Kick the backend → store sync loop. `init` is also called here
  // as a safety net — SessionTree's own useEffect already calls it
  // at mount, but if SyncStatusBadge mounts earlier (e.g. in a
  // future layout change) we still want watcher_status populated.
  useEffect(() => {
    const stop = startWatcherPolling();
    void init(); // fire-and-forget; init is idempotent
    return stop;
  }, [startWatcherPolling, init]);

  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const dotClass = autoSyncLive ? "bg-accent-green" : "bg-accent-red";
  const label = autoSyncLive ? "自动同步" : "同步已停止";
  const ageText = formatAge(last_scan_at_ms, now);
  const tooltip = watcherDirs.length
    ? `${label} · 监听: ${watcherDirs.join(", ")}`
    : label;

  return (
    <div
      data-testid="sync-status-badge"
      title={tooltip}
      className="inline-flex items-center gap-1.5 text-xs text-fg-muted"
    >
      <span
        className={`inline-block w-1.5 h-1.5 rounded-full ${dotClass} ${
          autoSyncLive ? "animate-pulse" : ""
        }`}
      />
      <span>{label}</span>
      <span>·</span>
      <span className="font-mono">{ageText}</span>
    </div>
  );
}

/// Pure helper — exported so SyncStatusBadge.test.tsx can exercise
/// the time-formatting branches without mounting React. Returns:
///   - "尚未扫描" when `lastScanMs` is null
///   - "{sec}s 前" when < 60s elapsed (sec clamped to ≥0)
///   - "{min}m 前" when < 60m elapsed
///   - "{hr}h 前"  otherwise
///
/// `now` is passed in (rather than read from `Date.now()`) so tests
/// can pin the clock and avoid flake on slow CI.
export function formatAge(lastScanMs: number | null, now: number): string {
  if (lastScanMs === null) return "尚未扫描";
  const sec = Math.max(0, Math.floor((now - lastScanMs) / 1000));
  if (sec < 60) return `${sec}s 前`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m 前`;
  const hr = Math.floor(min / 60);
  return `${hr}h 前`;
}