// src/components/SessionDetail.tsx — right panel: title + metadata + resume cmd
//
// v0.2.5: i18n — every user-visible string goes through `useTranslation`'s
// `t`. The three banners (sync / broken / delete-dialog) keep their
// existing 3-state semantics; only the copy changes. `formatTimestamp`
// now picks its locale off `i18n.language` so that English users see
// `7/4/2026, 12:34 PM` rather than the v0.2.x `2026/7/4 12:34`.

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionStore } from "../store/sessionStore";
import { SourceBadge } from "./SourceBadge";
import { MessageList } from "./MessageList";
import {
  deleteSession,
  fixOrphans,
  getConversation,
  getResumeCommand,
  syncNow as syncNowTauri,
  syncSessionLayer23,
  type Conversation,
  type DeleteReport,
  type FixOrphansReport,
  type SyncReport,
} from "../lib/tauri";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  Copy,
  Trash2,
  Folder,
  Clock,
  FileText,
  Hash,
  RefreshCw,
  ArrowLeftRight,
  Wrench,
  X,
} from "lucide-react";
import type { SourceLayer } from "../lib/types";
import { resolveTitle } from "../lib/display";

function formatTimestamp(ms: number, locale: string): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function SessionDetail() {
  const { t, i18n } = useTranslation();
  const selectedUuid = useSessionStore((s) => s.selectedUuid);
  const sessions = useSessionStore((s) => s.sessions);
  const session = useMemo(
    () => sessions.find((x) => x.uuid === selectedUuid) ?? null,
    [sessions, selectedUuid],
  );
  const [resumeCmd, setResumeCmd] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [conv, setConv] = useState<Conversation | null>(null);
  const [convLoading, setConvLoading] = useState(false);
  const [convError, setConvError] = useState<string | null>(null);
  // v0.2-alpha one-click L2↔L3 sync state. Three UI states only:
  //   idle    → no run yet (button shows with current missing[]).
  //   running → command in flight (spinner + disabled).
  //   done    → SyncReport received; refreshSessions in flight so
  //             sources.* tags update in place.
  const [syncRunning, setSyncRunning] = useState(false);
  const [syncReport, setSyncReport] = useState<SyncReport | null>(null);
  const [syncError, setSyncError] = useState<string | null>(null);
  // v0.2.1 — per-session "修复" + "删除" entry points.
  const [repairRunning, setRepairRunning] = useState(false);
  const [repairReport, setRepairReport] = useState<FixOrphansReport | null>(
    null,
  );
  const [repairError, setRepairError] = useState<string | null>(null);
  // 删除确认 dialog (native <dialog>). L1/L2 checkbox 默认勾,
  // L3 disabled + 说明文字. cursor_running 时按钮 disabled + 红字提示.
  const deleteDialogRef = useRef<HTMLDialogElement | null>(null);
  const [deleteL1Checked, setDeleteL1Checked] = useState(true);
  const [deleteL2Checked, setDeleteL2Checked] = useState(true);
  const [deleteRunning, setDeleteRunning] = useState(false);
  const [deleteReport, setDeleteReport] = useState<DeleteReport | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  useEffect(() => {
    if (!selectedUuid) {
      setConv(null);
      setConvError(null);
      return;
    }
    let cancelled = false;
    setConvLoading(true);
    setConvError(null);
    getConversation(selectedUuid)
      .then((c) => {
        if (!cancelled) setConv(c);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setConv(null);
          setConvError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!cancelled) setConvLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedUuid]);

  const sources = useMemo(() => {
    if (!session) return [];
    const out: Array<{ layer: SourceLayer; path: string }> = [];
    if (session.sources.mac) out.push({ layer: "mac", path: session.sources.mac.path });
    if (session.sources.linux_cli) out.push({ layer: "linux_cli", path: session.sources.linux_cli.path });
    if (session.sources.linux_desktop) out.push({ layer: "linux_desktop", path: session.sources.linux_desktop.path });
    return out;
  }, [session]);

  const primarySource = sources[0]?.layer ?? null;

  // ── v0.2-alpha sync: which layers are missing? ─────────────
  // Layer 2 (store.db) presence ≈ `sources.linux_cli` (post-#88, that
  // tag is only stamped when store.db is actually present, since L1
  // no longer stamps it). Layer 3 (state.vscdb composerData) presence
  // is the explicit `layer_3_present` flag. `missing` drives the sync
  // banner's copy + the (single) sync button label.
  const syncMissing = useMemo(() => {
    if (!session) return { missing: [] as Array<"L2" | "L3">, hasL2: false, hasL3: false, needsL3Refresh: false };
    const hasL2 = !!session.sources.linux_cli;
    const hasL3 = !!session.layer_3_present;
    const needsL3Refresh = !!session.layer_3_needs_refresh;
    const missing: Array<"L2" | "L3"> = [];
    if (!hasL2) missing.push("L2");
    if (!hasL3 || needsL3Refresh) missing.push("L3");
    return { missing, hasL2, hasL3, needsL3Refresh };
  }, [session]);

  // v0.2-alpha one-click L2↔L3 补层 sync. `cwd` is sourced from the
  // session's `project_path` (Layer 3 workspaceIdentifier when present,
  // otherwise empty → Rust side falls back to chats-dir scan). On
  // success we `syncNowTauri()` so the new `sources.linux_cli` /
  // `layer_3_present` flags propagate to the sidebar badge and the
  // missing[] banner disappears.
  const handleCopyResume = async () => {
    if (!session) return;
    const src = primarySource ?? "linux_cli";
    try {
      const cmd = await getResumeCommand(session.uuid, src);
      await writeText(cmd);
      setResumeCmd(cmd);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("copy failed:", e);
    }
  };

  const handleSync = async () => {
    if (!session) return;
    setSyncRunning(true);
    setSyncError(null);
    setSyncReport(null);
    try {
      const cwd = session.project_path || null;
      const report = await syncSessionLayer23(session.uuid, cwd);
      setSyncReport(report);
      // Whether we wrote anything, refresh so source tags reflect
      // new on-disk state. `cursor_running` skip still leaves state
      // unchanged, but the user expects a refresh either way.
      await syncNowTauri().catch(() => undefined);
    } catch (e: unknown) {
      setSyncError(e instanceof Error ? e.message : String(e));
    } finally {
      setSyncRunning(false);
    }
  };

  // v0.2.1 — 后端只有一个全量 fix_orphans 命令, 单条 UI 也调它.
  // 用户点一条 broken 之后, 整个 chats 目录都会被扫描 + 修. 这跟
  // 用户在 SessionTree 点 Wrench 的语义一致, 且实现成本小.
  const handleRepair = async () => {
    if (!session) return;
    setRepairRunning(true);
    setRepairError(null);
    setRepairReport(null);
    try {
      const report = await fixOrphans();
      setRepairReport(report);
      await syncNowTauri().catch(() => undefined);
    } catch (e: unknown) {
      setRepairError(e instanceof Error ? e.message : String(e));
    } finally {
      setRepairRunning(false);
    }
  };

  const openDeleteDialog = () => {
    setDeleteL1Checked(true);
    setDeleteL2Checked(true);
    setDeleteReport(null);
    setDeleteError(null);
    deleteDialogRef.current?.showModal();
  };

  const closeDeleteDialog = () => {
    if (deleteRunning) return;
    deleteDialogRef.current?.close();
  };

  const confirmDelete = async () => {
    if (!session) return;
    if (!deleteL1Checked && !deleteL2Checked) return;
    setDeleteRunning(true);
    setDeleteError(null);
    setDeleteReport(null);
    try {
      const slug = deleteL1Checked ? session.project_slug : null;
      const cwd = deleteL2Checked ? session.project_path || "" : "";
      const report = await deleteSession(
        session.uuid,
        cwd || null,
        slug || null,
      );
      setDeleteReport(report);
      await syncNowTauri().catch(() => undefined);
      // 关闭 dialog 仅在真的删了什么的时候 (避免 cursor_running
      // 之类的场景下悄悄把 dialog 关掉, 让用户没法看 reason).
      const deletedSomething =
        report.removed_l1 || report.removed_l2;
      if (deletedSomething) {
        deleteDialogRef.current?.close();
      }
    } catch (e: unknown) {
      setDeleteError(e instanceof Error ? e.message : String(e));
    } finally {
      setDeleteRunning(false);
    }
  };

  if (!selectedUuid || !session) {
    return (
      <div className="flex-1 flex items-center justify-center bg-bg-primary text-fg-muted text-sm">
        {t("detail.selectSessionHint")}
      </div>
    );
  }

  // Layer 2 path suffix: chats/<cwd-with-slashes-to-dashes>/<uuid>/,
  // or the "<md5(cwd)>" fallback when cwd is missing/empty. The
  // dialog renders this verbatim — split out so the JSX stays
  // readable.
  const layer2CwdSegment = session.project_path
    ? session.project_path
        .trim()
        .replace(/^\/+/, "")
        .replace(/\//g, "-")
    : null;

  return (
    <div className="flex-1 flex flex-col bg-bg-primary overflow-hidden">
      {/* Header */}
      <div className="px-6 py-4 border-b border-border">
        {/* v0.2-alpha: 跨端同步 banner. Shows whenever at least one
            of L2 / L3 is missing — drives the user toward the
            single button that fills them in one shot. Hidden when
            both layers are present (or sync just succeeded). */}
        {syncMissing.missing.length > 0 && (
          <div
            data-testid="sync-banner"
            className="mb-3 px-3 py-2 rounded-md bg-accent-blue/10 border border-accent-blue/40 text-fg-primary text-xs flex items-start gap-2"
          >
            <ArrowLeftRight size={14} className="text-accent-blue shrink-0 mt-px" />
            <div className="flex-1">
              <div className="font-semibold text-accent-blue">
                {syncMissing.needsL3Refresh && syncMissing.hasL3
                  ? t("detail.syncBanner.refreshL3")
                  : syncMissing.missing.length === 2
                  ? t("detail.syncBanner.missingBoth")
                  : syncMissing.missing.includes("L2")
                  ? t("detail.syncBanner.missingL2")
                  : t("detail.syncBanner.missingL3")}
              </div>
              <div className="text-fg-muted mt-0.5">
                {syncMissing.needsL3Refresh && syncMissing.hasL3 ? (
                  <span>{t("detail.syncBanner.hintRefreshL3")}</span>
                ) : (
                  <>
                {syncMissing.missing.includes("L2") && (
                  <span>{t("detail.syncBanner.hintL2")}</span>
                )}
                {syncMissing.missing.length === 2 && " "}
                {syncMissing.missing.includes("L3") && !syncMissing.needsL3Refresh && (
                  <span>{t("detail.syncBanner.hintL3")}</span>
                )}
                  </>
                )}
              </div>
              {syncReport && (
                <div className="mt-1 text-fg-secondary">
                  {syncReport.wrote_layer2 && (
                    <span>
                      {syncReport.root_blob_id
                        ? t("detail.syncBanner.wroteL2Root", {
                            root: syncReport.root_blob_id.slice(0, 8),
                          })
                        : t("detail.syncBanner.wroteL2NoRoot")}
                    </span>
                  )}
                  {syncReport.wrote_layer3 && (
                    <span className="ml-2">{t("detail.syncBanner.wroteL3")}</span>
                  )}
                  {syncReport.skipped.length > 0 && (
                    <span className="ml-2 text-accent-yellow">
                      {t("detail.syncBanner.skipped", {
                        items: syncReport.skipped.join(", "),
                      })}
                    </span>
                  )}
                  <span className="ml-2 text-fg-muted">
                    {t("detail.syncBanner.durationMs", {
                      ms: syncReport.duration_ms,
                    })}
                  </span>
                </div>
              )}
              {syncError && (
                <div className="mt-1 text-accent-red">
                  {t("common.error", { msg: syncError })}
                </div>
              )}
            </div>
            <button
              type="button"
              data-testid="sync-button"
              disabled={syncRunning}
              onClick={handleSync}
              className="px-2.5 py-1 rounded bg-accent-blue text-bg-primary font-semibold hover:bg-accent-blue/90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5 shrink-0"
            >
              {syncRunning ? (
                <>
                  <RefreshCw size={11} className="animate-spin" />
                  {t("detail.syncBanner.running")}
                </>
              ) : (
                <>
                  <ArrowLeftRight size={11} />
                  {syncMissing.needsL3Refresh && syncMissing.hasL3
                    ? t("detail.syncBanner.refillL3")
                    : syncMissing.missing.includes("L2")
                    ? t("detail.syncBanner.fillL2")
                    : syncMissing.missing.includes("L3")
                    ? t("detail.syncBanner.fillL3")
                    : t("detail.syncBanner.fillAll")}
                </>
              )}
            </button>
          </div>
        )}

        {/* Broken-state banner (above title so it can't be missed). */}
        {session.is_broken && (
          <div className="mb-3 px-3 py-2 rounded-md bg-accent-yellow/10 border border-accent-yellow/40 text-accent-yellow text-xs flex items-start gap-2">
            <span className="text-base leading-none shrink-0 mt-px">⚠</span>
            <div className="flex-1">
              <div className="font-semibold">
                {session.broken_reason ?? t("detail.brokenBanner.defaultReason")}
              </div>
              <div className="text-fg-muted mt-0.5">
                {t("detail.brokenBanner.hint")}
              </div>
              {repairReport && (
                <div className="mt-1 text-fg-secondary">
                  {repairReport.skipped.length > 0
                    ? t("detail.brokenBanner.scannedFixedSkip", {
                        scanned: repairReport.scanned,
                        fixed: repairReport.fixed.length,
                        skipped: repairReport.skipped.length,
                      })
                    : t("detail.brokenBanner.scannedFixed", {
                        scanned: repairReport.scanned,
                        fixed: repairReport.fixed.length,
                      })}
                  {repairReport.skipped.length > 0 && (
                    <span className="ml-2 text-accent-yellow">
                      {t("detail.brokenBanner.skipped", {
                        count: repairReport.skipped.length,
                      })}
                    </span>
                  )}
                </div>
              )}
              {repairError && (
                <div className="mt-1 text-accent-red">
                  {t("common.error", { msg: repairError })}
                </div>
              )}
            </div>
            <button
              type="button"
              data-testid="repair-button"
              disabled={repairRunning}
              onClick={handleRepair}
              className="px-2.5 py-1 rounded bg-accent-yellow text-bg-primary font-semibold hover:bg-accent-yellow/90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5 shrink-0"
              title={t("detail.brokenBanner.title")}
            >
              {repairRunning ? (
                <>
                  <RefreshCw size={11} className="animate-spin" />
                  {t("detail.brokenBanner.running")}
                </>
              ) : (
                <>
                  <Wrench size={11} />
                  {t("detail.brokenBanner.repair")}
                </>
              )}
            </button>
          </div>
        )}

        <div className="flex items-start gap-3">
          <div className="flex-1 min-w-0">
            <h2
              className={`text-base font-semibold truncate ${
                resolveTitle(session).isFallback
                  ? "text-fg-muted italic font-normal"
                  : "text-fg-primary"
              }`}
              title={resolveTitle(session).text}
            >
              {resolveTitle(session).text}
            </h2>
            {session.first_user_message_preview && session.name && (
              <p className="text-xs text-fg-secondary mt-1 line-clamp-2">
                {session.first_user_message_preview}
              </p>
            )}
          </div>
          <button
            type="button"
            data-testid="delete-button"
            onClick={openDeleteDialog}
            className="px-3 py-1.5 rounded-md bg-accent-red/15 border border-accent-red/30 text-accent-red text-xs font-medium hover:bg-accent-red/25"
            title={t("detail.actions.deleteTitle")}
          >
            <Trash2 size={12} className="inline mr-1" />
            {t("detail.actions.deleteSession")}
          </button>
        </div>

        {/* Metadata grid */}
        <div className="mt-3 grid grid-cols-2 gap-x-6 gap-y-1.5 text-xs">
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <Clock size={12} className="text-fg-muted" />
            <span>{t("detail.metadata.lastUpdated")}</span>
            <span className="text-fg-primary font-mono">
              {formatTimestamp(session.last_updated_at, i18n.language)}
            </span>
          </div>
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <Folder size={12} className="text-fg-muted" />
            <span>{t("detail.metadata.project")}</span>
            <span className="text-fg-primary font-mono truncate">
              {session.project_slug}
            </span>
          </div>
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <Hash size={12} className="text-fg-muted" />
            <span>{t("detail.metadata.uuid")}</span>
            <span className="text-fg-primary font-mono truncate" title={session.uuid}>
              {session.uuid}
            </span>
          </div>
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <FileText size={12} className="text-fg-muted" />
            <span>{t("detail.metadata.bubbleCount")}</span>
            <span className="text-fg-primary font-mono">{session.bubble_count}</span>
          </div>
        </div>

        {/* Sources */}
        {sources.length > 0 && (
          <div className="mt-3 flex items-center gap-1.5 flex-wrap">
            <span className="text-xs text-fg-muted">
              {t("detail.metadata.source")}
            </span>
            {sources.map((s) => (
              <SourceBadge key={s.layer} source={s.layer} size="md" />
            ))}
          </div>
        )}

        {/* Resume command */}
        <div className="mt-3 flex items-center gap-2 px-3 py-2 rounded-md bg-bg-tertiary border border-border">
          <span className="text-xs text-fg-muted font-mono whitespace-nowrap">
            $&nbsp;
          </span>
          <code className="flex-1 text-xs text-fg-primary font-mono truncate">
            {resumeCmd ??
              (primarySource === "linux_cli"
                ? `cursor-agent --resume ${session.uuid}`
                : `open -a Cursor --args --resume ${session.uuid}`)}
          </code>
          <button
            onClick={handleCopyResume}
            className="p-1.5 rounded hover:bg-bg-hover text-fg-secondary hover:text-fg-primary"
            title={t("detail.actions.copyResumeTitle")}
          >
            <Copy size={12} />
          </button>
        </div>
        {copied && (
          <div className="mt-1.5 text-xs text-accent-green">
            {t("common.copied")}
          </div>
        )}
      </div>

      {/* Conversation — v0.2.2: thin MessageList wrapper (sticky
          header + 3-state copy + scroll-to-bottom + stable key
          forwarding). See MessageList.tsx for the rationale. */}
      <MessageList
        conv={conv}
        loading={convLoading}
        error={convError}
      />

      {/* v0.2.1 — 删除会话确认 dialog. 原生 <dialog>, 不引新依赖.
          L1 (JSONL) + L2 (store.db) 默认勾, L3 (state.vscdb) 强制
          disabled + 解释"由 Cursor 管理". cursor_running 时整个
          确认按钮 disabled, 让用户先关 Cursor. */}
      <dialog
        ref={deleteDialogRef}
        data-testid="delete-dialog"
        className="p-0 rounded-lg border border-border bg-bg-primary text-fg-primary shadow-2xl w-[min(480px,90vw)]"
        onCancel={(e) => {
          if (deleteRunning) e.preventDefault();
        }}
      >
        <div className="px-5 py-3 border-b border-border flex items-center justify-between">
          <div className="font-semibold text-sm flex items-center gap-2">
            <Trash2 size={14} className="text-accent-red" />
            {t("detail.deleteDialog.title")}
          </div>
          <button
            type="button"
            data-testid="delete-dialog-close"
            onClick={closeDeleteDialog}
            disabled={deleteRunning}
            className="p-1 rounded hover:bg-bg-hover text-fg-muted disabled:opacity-40"
            title={t("common.close")}
          >
            <X size={14} />
          </button>
        </div>
        <div className="px-5 py-4 text-sm space-y-3">
          <div className="text-fg-secondary">
            {t("detail.deleteDialog.body", {
              uuid: session.uuid.slice(0, 8),
            })}
          </div>

          <label className="flex items-start gap-2 px-3 py-2 rounded-md bg-bg-tertiary border border-border cursor-pointer">
            <input
              type="checkbox"
              data-testid="delete-l1-checkbox"
              checked={deleteL1Checked}
              onChange={(e) => setDeleteL1Checked(e.target.checked)}
              disabled={deleteRunning}
              className="mt-1 shrink-0"
            />
            <div className="flex-1 min-w-0">
              <div className="font-mono text-xs text-fg-primary">
                {t("detail.deleteDialog.layerL1")}
              </div>
              <div className="text-xs text-fg-muted mt-0.5 break-all">
                {t("detail.deleteDialog.layerL1Path", {
                  slug: session.project_slug,
                  uuid: session.uuid,
                })}
              </div>
            </div>
          </label>

          <label className="flex items-start gap-2 px-3 py-2 rounded-md bg-bg-tertiary border border-border cursor-pointer">
            <input
              type="checkbox"
              data-testid="delete-l2-checkbox"
              checked={deleteL2Checked}
              onChange={(e) => setDeleteL2Checked(e.target.checked)}
              disabled={deleteRunning}
              className="mt-1 shrink-0"
            />
            <div className="flex-1 min-w-0">
              <div className="font-mono text-xs text-fg-primary">
                {t("detail.deleteDialog.layerL2")}
              </div>
              <div className="text-xs text-fg-muted mt-0.5 break-all">
                {layer2CwdSegment
                  ? t("detail.deleteDialog.layerL2Path", {
                      cwd: layer2CwdSegment,
                      uuid: session.uuid,
                    })
                  : t("detail.deleteDialog.layerL2PathFallback", {
                      uuid: session.uuid,
                    })}
              </div>
            </div>
          </label>

          <label className="flex items-start gap-2 px-3 py-2 rounded-md bg-bg-tertiary/40 border border-border opacity-60 cursor-not-allowed">
            <input
              type="checkbox"
              data-testid="delete-l3-checkbox"
              checked={false}
              disabled
              className="mt-1 shrink-0"
            />
            <div className="flex-1 min-w-0">
              <div className="font-mono text-xs text-fg-muted">
                {t("detail.deleteDialog.layerL3")}
              </div>
              <div className="text-xs text-fg-muted mt-0.5">
                {t("detail.deleteDialog.layerL3Note")}
              </div>
            </div>
          </label>

          {deleteReport?.cursor_running && (
            <div className="text-xs text-accent-red bg-accent-red/10 border border-accent-red/30 rounded px-2 py-1.5">
              {t("detail.deleteDialog.running", {
                count: deleteReport.running_processes.length,
              })}
            </div>
          )}
          {deleteReport && !deleteReport.cursor_running && (
            <div className="text-xs text-fg-secondary bg-bg-tertiary border border-border rounded px-2 py-1.5">
              <div>
                {deleteReport.removed_l1
                  ? t("detail.deleteDialog.reportL1Removed")
                  : t("detail.deleteDialog.reportL1Skipped", {
                      reason:
                        deleteReport.skipped_l1 ?? t("detail.deleteDialog.skippedUnknown"),
                    })}
              </div>
              <div>
                {deleteReport.removed_l2
                  ? t("detail.deleteDialog.reportL2Removed")
                  : t("detail.deleteDialog.reportL2Skipped", {
                      reason:
                        deleteReport.skipped_l2 ?? t("detail.deleteDialog.skippedUnknown"),
                    })}
              </div>
            </div>
          )}
          {deleteError && (
            <div className="text-xs text-accent-red">
              {t("common.error", { msg: deleteError })}
            </div>
          )}
        </div>
        <div className="px-5 py-3 border-t border-border flex items-center justify-end gap-2">
          <button
            type="button"
            onClick={closeDeleteDialog}
            disabled={deleteRunning}
            className="px-3 py-1.5 rounded-md bg-bg-tertiary border border-border text-xs hover:bg-bg-hover disabled:opacity-50"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            data-testid="delete-confirm"
            onClick={confirmDelete}
            disabled={
              deleteRunning ||
              (!deleteL1Checked && !deleteL2Checked) ||
              !!deleteReport?.cursor_running
            }
            className="px-3 py-1.5 rounded-md bg-accent-red text-bg-primary text-xs font-semibold hover:bg-accent-red/90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
          >
            {deleteRunning ? (
              <>
                <RefreshCw size={11} className="animate-spin" />
                {t("detail.deleteDialog.deleting")}
              </>
            ) : (
              <>
                <Trash2 size={11} />
                {t("detail.deleteDialog.confirm")}
              </>
            )}
          </button>
        </div>
      </dialog>
    </div>
  );
}
