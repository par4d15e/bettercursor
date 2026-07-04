// src/components/SessionTree.tsx — left panel: project groups → sessions

import { useEffect, useState } from "react";
import { useSessionStore, useGroupedSessions, SORT_LABELS } from "../store/sessionStore";
import { SourceBadge } from "./SourceBadge";
import { BrokenBadge } from "./BrokenBadge";
import { SyncNowButton } from "./SyncNowButton";
import { SyncStatusBadge } from "./SyncStatusBadge";
import type { CanonicalSession, SourceLayer } from "../lib/types";
import { resolveTitle } from "../lib/display";
import { fixOrphans, syncNow as syncNowTauri, type FixOrphansReport } from "../lib/tauri";
import {
  ChevronLeft,
  ChevronDown,
  ChevronRight,
  Search,
  ArrowUpDown,
  ListFilter,
  Wrench,
  CheckCircle2,
  AlertTriangle,
} from "lucide-react";

function detectSource(s: CanonicalSession): SourceLayer | null {
  if (s.sources.linux_cli) return "linux_cli";
  if (s.sources.mac) return "mac";
  if (s.sources.linux_desktop) return "linux_desktop";
  return null;
}

export function SessionTree() {
  const grouped = useGroupedSessions();
  const selected = useSessionStore((s) => s.selectedUuid);
  const setSelected = useSessionStore((s) => s.setSelected);
  const search = useSessionStore((s) => s.search);
  const setSearch = useSessionStore((s) => s.setSearch);
  const loading = useSessionStore((s) => s.loading);
  const total = useSessionStore((s) => s.sessions.length);
  const error = useSessionStore((s) => s.error);
  const init = useSessionStore((s) => s.init);
  const sortMode = useSessionStore((s) => s.sortMode);
  const cycleSortMode = useSessionStore((s) => s.cycleSortMode);

  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  // v0.2.1 — 头部 Wrench 按钮 (批量修复 orphan). 状态独立于
  // SessionDetail 的单条修复按钮 (两边调同一个 fixOrphans).
  // `orphanToast` 是 4s 自动消失的横幅.
  const [fixingOrphans, setFixingOrphans] = useState(false);
  const [orphanToast, setOrphanToast] = useState<
    | { kind: "ok" | "err"; text: string; report?: FixOrphansReport }
    | null
  >(null);

  useEffect(() => {
    init();
  }, [init]);

  // 4s 自动消失. useEffect 挂在新 toast 上, 卸载时清理 timer.
  useEffect(() => {
    if (!orphanToast) return;
    const id = window.setTimeout(() => setOrphanToast(null), 4000);
    return () => window.clearTimeout(id);
  }, [orphanToast]);

  const handleFixOrphans = async () => {
    setFixingOrphans(true);
    setOrphanToast(null);
    try {
      const report = await fixOrphans();
      await syncNowTauri().catch(() => undefined);
      setOrphanToast({
        kind: "ok",
        text: `扫描 ${report.scanned} 条 orphan, 修复 ${report.fixed.length} 条${
          report.skipped.length > 0
            ? `, 跳过 ${report.skipped.length} 条`
            : ""
        }`,
        report,
      });
    } catch (e: unknown) {
      setOrphanToast({
        kind: "err",
        text: `修复失败: ${e instanceof Error ? e.message : String(e)}`,
      });
    } finally {
      setFixingOrphans(false);
    }
  };

  const toggle = (slug: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(slug)) next.delete(slug);
      else next.add(slug);
      return next;
    });
  };

  return (
    <div className="flex flex-col h-full bg-bg-secondary border-r border-border text-fg-primary">
      {/* Header / toolbar — always visible */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border bg-bg-secondary">
        <button
          className="p-1 rounded hover:bg-bg-hover text-fg-secondary"
          title="返回"
        >
          <ChevronLeft size={16} />
        </button>
        <h1 className="text-sm font-semibold text-fg-primary">会话管理</h1>
        {/* v0.2.3: replaces the old "LIVE" badge — now shows the
            watcher state + "Xs 前" counter. Polls watcher_status
            every 5s, ticks the counter every 1s locally. */}
        <span className="ml-auto">
          <SyncStatusBadge />
        </span>
        <span className="text-[10px] text-fg-muted font-mono">v0.2.3</span>
      </div>

      <div className="flex items-center gap-2 px-3 py-2 border-b border-border text-xs text-fg-secondary">
        <span>会话列表</span>
        <span className="px-1.5 py-0.5 rounded bg-bg-tertiary text-fg-primary font-mono">
          {total}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <button
            className="p-1 rounded hover:bg-bg-hover"
            title="多选"
            disabled
          >
            <ListFilter size={14} />
          </button>
          <button
            className="p-1 rounded hover:bg-bg-hover flex items-center gap-1 px-1.5"
            onClick={cycleSortMode}
            title={`排序: ${SORT_LABELS[sortMode]} (点击切换)`}
          >
            <ArrowUpDown size={14} />
            <span className="text-[10px]">{SORT_LABELS[sortMode]}</span>
          </button>
          <button
            data-testid="fix-orphans-button"
            className="p-1 rounded hover:bg-bg-hover"
            onClick={handleFixOrphans}
            title="修复所有 Layer 2 orphan session (latestRootBlobId 是空字符串). 已自动备份 store.db."
            disabled={fixingOrphans}
          >
            <Wrench
              size={14}
              className={fixingOrphans ? "animate-spin" : ""}
            />
          </button>
          {/* v0.2.3: extracted from inline button. Same behavior
              (RefreshCw + spinner), but the icon is now sourced from
              its own component so the click-debounce / loading
              derivation lives in one place. */}
          <SyncNowButton />
        </div>
      </div>

      {/* v0.2.1 — fix_orphans toast. 顶部居中, 跟 Search 与 List
          之间无遮挡; 4s 自动消失. ok 时 acc-green, err 时 accent-red. */}
      {orphanToast && (
        <div
          data-testid="fix-orphans-toast"
          role="status"
          className={`mx-3 mt-2 px-2.5 py-1.5 rounded border text-xs flex items-start gap-2 ${
            orphanToast.kind === "ok"
              ? "bg-accent-green/10 border-accent-green/40 text-accent-green"
              : "bg-accent-red/10 border-accent-red/40 text-accent-red"
          }`}
        >
          {orphanToast.kind === "ok" ? (
            <CheckCircle2 size={12} className="shrink-0 mt-px" />
          ) : (
            <AlertTriangle size={12} className="shrink-0 mt-px" />
          )}
          <div className="flex-1">{orphanToast.text}</div>
        </div>
      )}

      {/* Search */}
      <div className="px-3 py-2 border-b border-border">
        <div className="flex items-center gap-2 px-2 py-1.5 rounded-md bg-bg-tertiary border border-border focus-within:border-border-strong">
          <Search size={14} className="text-fg-muted" />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索会话名 / 项目 / 内容"
            className="flex-1 bg-transparent text-xs text-fg-primary outline-none placeholder:text-fg-muted"
          />
        </div>
      </div>

      {/* Error banner */}
      {error && (
        <div className="mx-3 mt-2 px-2 py-1.5 rounded bg-accent-red/10 border border-accent-red/30 text-xs text-accent-red">
          {error}
        </div>
      )}

      {/* Tree */}
      <div className="flex-1 overflow-y-auto py-1">
        {/* Provider root: "Cursor" */}
        <div className="px-3 py-1.5 flex items-center gap-1.5 text-xs font-semibold text-fg-primary">
          <ChevronDown size={12} className="text-fg-muted" />
          <span>Cursor</span>
          <span className="ml-auto px-1.5 py-0.5 rounded bg-bg-tertiary text-fg-secondary font-mono">
            {total}
          </span>
        </div>

        {/* Project groups */}
        {grouped.map(({ slug, sessions }) => {
          const isCollapsed = collapsed.has(slug);
          return (
            <div key={slug}>
              <button
                onClick={() => toggle(slug)}
                className="w-full px-3 py-1.5 flex items-center gap-1.5 text-xs text-fg-secondary hover:bg-bg-hover"
              >
                {isCollapsed ? (
                  <ChevronRight size={12} />
                ) : (
                  <ChevronDown size={12} />
                )}
                <span className="font-mono">{slug}</span>
                <span className="ml-auto px-1.5 py-0.5 rounded bg-bg-tertiary text-fg-muted font-mono">
                  {sessions.length}
                </span>
              </button>
              {!isCollapsed &&
                sessions.map((sess) => {
                  const src = detectSource(sess);
                  const isSelected = sess.uuid === selected;
                  return (
                    <button
                      key={sess.uuid}
                      onClick={() => setSelected(sess.uuid)}
                      className={`w-full pl-8 pr-3 py-1.5 flex items-center gap-2 text-xs hover:bg-bg-hover ${
                        isSelected
                          ? "bg-bg-hover border-l-2 border-accent-blue"
                          : ""
                      }`}
                    >
                      {(() => {
                        const t = resolveTitle(sess);
                        return (
                          <span
                            className={`truncate flex-1 text-left ${
                              t.isFallback
                                ? "text-fg-muted italic"
                                : "text-fg-primary"
                            }`}
                            title={t.text}
                          >
                            {t.text}
                          </span>
                        );
                      })()}
                      {sess.is_broken && (
                        <BrokenBadge reason={sess.broken_reason} size="sm" />
                      )}
                      {src && <SourceBadge source={src} />}
                    </button>
                  );
                })}
            </div>
          );
        })}

        {grouped.length === 0 && !loading && (
          <div className="px-3 py-8 text-center text-xs text-fg-muted">
            {total === 0
              ? "暂无 session. 点击刷新按钮重新扫描."
              : "没有匹配当前搜索的 session."}
          </div>
        )}
      </div>
    </div>
  );
}
