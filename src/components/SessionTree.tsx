// src/components/SessionTree.tsx — left panel: project groups → sessions
//
// v0.2.5: i18n — every visible string now goes through `useTranslation`'s
// `t`, including the dynamic `SORT_LABELS` lookup (formerly a static
// `Record<SortMode, string>` exported from `sessionStore`; we now
// compute it per-render via `getSortLabels(t)`). Settings (gear icon)
// in the header opens `<SettingsDialog />` for language / sync / conflicts.

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  useSessionStore,
  useGroupedSessions,
  getSortLabels,
  ancestorSessionIds,
  countSessionTreeNodes,
  type SortMode,
  type SessionTreeNode,
} from "../store/sessionStore";
import { SourceBadge } from "./SourceBadge";
import { BrokenBadge } from "./BrokenBadge";
import { SyncNowButton } from "./SyncNowButton";
import { SyncStatusBadge } from "./SyncStatusBadge";
import { SettingsButton } from "./SettingsButton";
import { SettingsDialog } from "./SettingsDialog";
import type { CanonicalSession, SourceLayer } from "../lib/types";
import { resolveTitle } from "../lib/display";
import { fixOrphans, syncNow as syncNowTauri, type FixOrphansReport } from "../lib/tauri";
import {
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  ChevronsUpDown,
  Search,
  ArrowUpDown,
  ListFilter,
  Wrench,
  CheckCircle2,
  AlertTriangle,
} from "lucide-react";

function detectSource(s: CanonicalSession): SourceLayer | null {
  if (s.created_endpoint) return s.created_endpoint;
  if (s.sources.linux_cli) return "linux_cli";
  if (s.sources.mac) return "mac";
  if (s.sources.linux_desktop) return "linux_desktop";
  return null;
}

function sessionIndentClass(depth: number): string {
  if (depth <= 0) return "pl-8";
  if (depth === 1) return "pl-12";
  if (depth === 2) return "pl-16";
  return "pl-20";
}

interface SessionNodeListProps {
  nodes: SessionTreeNode[];
  depth: number;
  selected: string | null;
  expandedSubagents: Set<string>;
  onToggleSubagents: (parentUuid: string) => void;
  onSelect: (uuid: string) => void;
}

function SessionNodeList({
  nodes,
  depth,
  selected,
  expandedSubagents,
  onToggleSubagents,
  onSelect,
}: SessionNodeListProps) {
  const { t } = useTranslation();

  return (
    <>
      {nodes.map((node) => {
        const { session: sess, children } = node;
        const src = detectSource(sess);
        const isSelected = sess.uuid === selected;
        const hasChildren = children.length > 0;
        const subagentsExpanded =
          hasChildren && expandedSubagents.has(sess.uuid);

        return (
          <div key={sess.uuid}>
            <div
              className={`w-full pr-3 py-1.5 flex items-center gap-1 text-xs hover:bg-bg-hover ${
                isSelected ? "bg-bg-hover border-l-2 border-accent-blue" : ""
              } ${sessionIndentClass(depth)}`}
            >
              {hasChildren ? (
                <button
                  type="button"
                  data-testid={`toggle-subagents-${sess.uuid.slice(0, 8)}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleSubagents(sess.uuid);
                  }}
                  className="p-0.5 rounded hover:bg-bg-tertiary text-fg-muted shrink-0"
                  title={
                    subagentsExpanded
                      ? t("tree.subagentCollapse")
                      : t("tree.subagentExpand", { count: children.length })
                  }
                >
                  {subagentsExpanded ? (
                    <ChevronDown size={12} />
                  ) : (
                    <ChevronRight size={12} />
                  )}
                </button>
              ) : (
                <span className="w-4 shrink-0" aria-hidden />
              )}
              <button
                type="button"
                onClick={() => onSelect(sess.uuid)}
                className="flex-1 min-w-0 flex items-center gap-2 text-left"
              >
                {(() => {
                  const title = resolveTitle(sess);
                  return (
                    <span
                      className={`truncate flex-1 ${
                        title.isFallback
                          ? "text-fg-muted italic"
                          : "text-fg-primary"
                      }`}
                      title={title.text}
                    >
                      {title.text}
                    </span>
                  );
                })()}
                {sess.is_broken && (
                  <BrokenBadge reason={sess.broken_reason} size="sm" />
                )}
                {sess.is_subagent && (
                  <span
                    className="shrink-0 px-1 py-0.5 rounded text-[10px] font-medium bg-accent-yellow/15 text-accent-yellow border border-accent-yellow/30"
                    title={t("tree.subagentBadge")}
                  >
                    {t("tree.subagentBadge")}
                  </span>
                )}
                {hasChildren && !subagentsExpanded && (
                  <span className="shrink-0 px-1 py-0.5 rounded text-[10px] font-mono bg-bg-tertiary text-fg-muted border border-border">
                    +{children.length}
                  </span>
                )}
                {src && <SourceBadge source={src} />}
              </button>
            </div>
            {hasChildren && subagentsExpanded && (
              <SessionNodeList
                nodes={children}
                depth={depth + 1}
                selected={selected}
                expandedSubagents={expandedSubagents}
                onToggleSubagents={onToggleSubagents}
                onSelect={onSelect}
              />
            )}
          </div>
        );
      })}
    </>
  );
}

export function SessionTree() {
  const { t } = useTranslation();
  const SORT_LABELS = getSortLabels(t);

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
  /// Parents whose subagent children are visible. Empty = all subagent nests collapsed.
  const [expandedSubagents, setExpandedSubagents] = useState<Set<string>>(
    new Set(),
  );

  // v0.2.1 — 头部 Wrench 按钮 (批量修复 orphan). 状态独立于
  // SessionDetail 的单条修复按钮 (两边调同一个 fixOrphans).
  // `orphanToast` 是 4s 自动消失的横幅.
  const [fixingOrphans, setFixingOrphans] = useState(false);
  const [orphanToast, setOrphanToast] = useState<
    | { kind: "ok" | "err"; text: string; report?: FixOrphansReport }
    | null
  >(null);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    init();
  }, [init]);

  // Reveal ancestors when a nested subagent is selected from outside the tree.
  useEffect(() => {
    if (!selected) return;
    const ancestors = ancestorSessionIds(grouped, selected);
    if (ancestors.length === 0) return;
    setExpandedSubagents((prev) => {
      const next = new Set(prev);
      let changed = false;
      for (const id of ancestors) {
        if (!next.has(id)) {
          next.add(id);
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [selected, grouped]);

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
        text:
          report.skipped.length > 0
            ? t("tree.fixOrphansToast.successWithSkip", {
                scanned: report.scanned,
                fixed: report.fixed.length,
                skipped: report.skipped.length,
              })
            : t("tree.fixOrphansToast.successNoSkip", {
                scanned: report.scanned,
                fixed: report.fixed.length,
              }),
        report,
      });
    } catch (e: unknown) {
      setOrphanToast({
        kind: "err",
        text: t("tree.fixOrphansToast.failed", {
          msg: e instanceof Error ? e.message : String(e),
        }),
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

  const toggleSubagents = (parentUuid: string) => {
    setExpandedSubagents((prev) => {
      const next = new Set(prev);
      if (next.has(parentUuid)) next.delete(parentUuid);
      else next.add(parentUuid);
      return next;
    });
  };

  const allSlugs = grouped.map((g) => g.slug);
  const allCollapsed =
    allSlugs.length > 0 && allSlugs.every((slug) => collapsed.has(slug));

  const toggleAllGroups = () => {
    if (allCollapsed) {
      setCollapsed(new Set());
    } else {
      setCollapsed(new Set(allSlugs));
    }
  };

  // Pull the current `sortMode`'s label out so the toolbar button can
  // read it without a re-derivation in JSX (and so the `title`
  // attribute stays in sync if the user cycles the sort).
  const sortLabel = SORT_LABELS[sortMode satisfies SortMode];

  return (
    <div className="flex flex-col h-full bg-bg-secondary border-r border-border text-fg-primary">
      {/* Header / toolbar — always visible */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border bg-bg-secondary">
        <h1 className="text-sm font-semibold text-fg-primary">
          {t("tree.header.title")}
        </h1>
        <span className="ml-auto inline-flex items-center gap-1.5">
          <SyncStatusBadge />
          <SettingsButton onClick={() => setSettingsOpen(true)} />
        </span>
        <span className="text-[10px] text-fg-muted font-mono">
          {t("tree.header.version")}
        </span>
      </div>

      <div className="flex items-center gap-2 px-3 py-2 border-b border-border text-xs text-fg-secondary">
        <span>{t("tree.toolbar.sessionCount")}</span>
        <span className="px-1.5 py-0.5 rounded bg-bg-tertiary text-fg-primary font-mono">
          {total}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <button
            type="button"
            data-testid="toggle-all-groups-button"
            className="p-1 rounded hover:bg-bg-hover disabled:opacity-40"
            title={
              allCollapsed
                ? t("tree.toolbar.expandAll")
                : t("tree.toolbar.collapseAll")
            }
            onClick={toggleAllGroups}
            disabled={allSlugs.length === 0}
          >
            {allCollapsed ? (
              <ChevronsDownUp size={14} />
            ) : (
              <ChevronsUpDown size={14} />
            )}
          </button>
          <button
            className="p-1 rounded hover:bg-bg-hover"
            title={t("tree.toolbar.multiSelect")}
            disabled
          >
            <ListFilter size={14} />
          </button>
          <button
            className="p-1 rounded hover:bg-bg-hover flex items-center gap-1 px-1.5"
            onClick={cycleSortMode}
            title={t("tree.toolbar.sortBy", { mode: sortLabel })}
          >
            <ArrowUpDown size={14} />
            <span className="text-[10px]">{sortLabel}</span>
          </button>
          <button
            data-testid="fix-orphans-button"
            className="p-1 rounded hover:bg-bg-hover"
            onClick={handleFixOrphans}
            title={t("tree.toolbar.fixOrphans")}
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
            placeholder={t("tree.search.placeholder")}
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
        {/* Provider root: "Cursor" (brand name, intentionally untranslated) */}
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
          const sessionCount = countSessionTreeNodes(sessions);
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
                  {sessionCount}
                </span>
              </button>
              {!isCollapsed && (
                <SessionNodeList
                  nodes={sessions}
                  depth={0}
                  selected={selected}
                  expandedSubagents={expandedSubagents}
                  onToggleSubagents={toggleSubagents}
                  onSelect={setSelected}
                />
              )}
            </div>
          );
        })}

        {grouped.length === 0 && !loading && (
          <div className="px-3 py-8 text-center text-xs text-fg-muted">
            {total === 0
              ? t("tree.empty.noSessions")
              : t("tree.empty.noMatch")}
          </div>
        )}
      </div>
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  );
}
