// src/components/SessionDetail.tsx — right panel: title + metadata + resume cmd

import { useEffect, useMemo, useRef, useState } from "react";
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

function formatTimestamp(ms: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function SessionDetail() {
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
    if (!session) return { missing: [] as Array<"L2" | "L3">, hasL2: false, hasL3: false };
    const hasL2 = !!session.sources.linux_cli;
    const hasL3 = !!session.layer_3_present;
    const missing: Array<"L2" | "L3"> = [];
    if (!hasL2) missing.push("L2");
    if (!hasL3) missing.push("L3");
    return { missing, hasL2, hasL3 };
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
        ← 在左侧选择一条 session
      </div>
    );
  }

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
                {syncMissing.missing.length === 2
                  ? "两端都看不到这条 session"
                  : syncMissing.missing.includes("L2")
                  ? "cursor-agent 看不到这条 session (缺 Layer 2)"
                  : "Desktop Sidebar 看不到这条 session (缺 Layer 3)"}
              </div>
              <div className="text-fg-muted mt-0.5">
                {syncMissing.missing.includes("L2") && (
                  <span>
                    补 Layer 2 后 <code className="font-mono">cursor-agent --resume</code>{" "}
                    才能进入.
                  </span>
                )}
                {syncMissing.missing.length === 2 && " "}
                {syncMissing.missing.includes("L3") && (
                  <span>
                    补 Layer 3 后 Cursor Desktop Sidebar 才会显示.
                  </span>
                )}
              </div>
              {syncReport && (
                <div className="mt-1 text-fg-secondary">
                  {syncReport.wrote_layer2 && (
                    <span>
                      ✓ 写入 store.db
                      {syncReport.root_blob_id
                        ? ` (root=${syncReport.root_blob_id.slice(0, 8)}…)`
                        : ""}
                    </span>
                  )}
                  {syncReport.wrote_layer3 && (
                    <span className="ml-2">✓ 写入 state.vscdb</span>
                  )}
                  {syncReport.skipped.length > 0 && (
                    <span className="ml-2 text-accent-yellow">
                      跳过: {syncReport.skipped.join(", ")}
                    </span>
                  )}
                  <span className="ml-2 text-fg-muted">
                    ({syncReport.duration_ms} ms)
                  </span>
                </div>
              )}
              {syncError && (
                <div className="mt-1 text-accent-red">错误: {syncError}</div>
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
                  同步中…
                </>
              ) : (
                <>
                  <ArrowLeftRight size={11} />
                  {syncMissing.missing.includes("L2")
                    ? "补 Layer 2"
                    : syncMissing.missing.includes("L3")
                    ? "补 Layer 3"
                    : "一键同步"}
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
                {session.broken_reason ?? "该 session 数据不完整"}
              </div>
              <div className="text-fg-muted mt-0.5">
                对应{" "}
                <code className="font-mono">cursor-agent --resume</code>{" "}
                命令可能失败. 点"修复 Layer 2"自动把 latestRootBlobId 填上
                (会留 .backup).
              </div>
              {repairReport && (
                <div className="mt-1 text-fg-secondary">
                  ✓ 扫描 {repairReport.scanned} 条, 修复{" "}
                  {repairReport.fixed.length} 条
                  {repairReport.skipped.length > 0 && (
                    <span className="ml-2 text-accent-yellow">
                      跳过: {repairReport.skipped.length} 条
                    </span>
                  )}
                </div>
              )}
              {repairError && (
                <div className="mt-1 text-accent-red">
                  错误: {repairError}
                </div>
              )}
            </div>
            <button
              type="button"
              data-testid="repair-button"
              disabled={repairRunning}
              onClick={handleRepair}
              className="px-2.5 py-1 rounded bg-accent-yellow text-bg-primary font-semibold hover:bg-accent-yellow/90 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5 shrink-0"
              title="扫描所有 chats/<md5>/<uuid>/store.db, 把 latestRootBlobId 是空字符串的修上. 已自动备份."
            >
              {repairRunning ? (
                <>
                  <RefreshCw size={11} className="animate-spin" />
                  修复中…
                </>
              ) : (
                <>
                  <Wrench size={11} />
                  修复 Layer 2
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
            title="删除 Layer 1 + Layer 2 存储 (L3 由 Cursor Desktop 自己管理, 暂不删)"
          >
            <Trash2 size={12} className="inline mr-1" />
            删除会话
          </button>
        </div>

        {/* Metadata grid */}
        <div className="mt-3 grid grid-cols-2 gap-x-6 gap-y-1.5 text-xs">
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <Clock size={12} className="text-fg-muted" />
            <span>最后更新:</span>
            <span className="text-fg-primary font-mono">
              {formatTimestamp(session.last_updated_at)}
            </span>
          </div>
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <Folder size={12} className="text-fg-muted" />
            <span>项目:</span>
            <span className="text-fg-primary font-mono truncate">
              {session.project_slug}
            </span>
          </div>
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <Hash size={12} className="text-fg-muted" />
            <span>UUID:</span>
            <span className="text-fg-primary font-mono truncate" title={session.uuid}>
              {session.uuid}
            </span>
          </div>
          <div className="flex items-center gap-1.5 text-fg-secondary">
            <FileText size={12} className="text-fg-muted" />
            <span>bubble 数:</span>
            <span className="text-fg-primary font-mono">{session.bubble_count}</span>
          </div>
        </div>

        {/* Sources */}
        {sources.length > 0 && (
          <div className="mt-3 flex items-center gap-1.5 flex-wrap">
            <span className="text-xs text-fg-muted">来源:</span>
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
            title="复制到剪贴板"
          >
            <Copy size={12} />
          </button>
        </div>
        {copied && (
          <div className="mt-1.5 text-xs text-accent-green">✓ 已复制到剪贴板</div>
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
            删除 session
          </div>
          <button
            type="button"
            data-testid="delete-dialog-close"
            onClick={closeDeleteDialog}
            disabled={deleteRunning}
            className="p-1 rounded hover:bg-bg-hover text-fg-muted disabled:opacity-40"
            title="关闭"
          >
            <X size={14} />
          </button>
        </div>
        <div className="px-5 py-4 text-sm space-y-3">
          <div className="text-fg-secondary">
            即将从磁盘删除 session{" "}
            <code className="font-mono text-fg-primary">
              {session.uuid.slice(0, 8)}…
            </code>
            . 至少勾选一层才能确认, 默认 L1 + L2 都删 (Cursor-agent 看不到的层).
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
                Layer 1 (JSONL)
              </div>
              <div className="text-xs text-fg-muted mt-0.5 break-all">
                ~/.cursor/projects/{session.project_slug}/agent-transcripts/
                {session.uuid}/
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
                Layer 2 (store.db)
              </div>
              <div className="text-xs text-fg-muted mt-0.5 break-all">
                ~/.cursor/chats/{session.project_path
                  ? `${session.project_path
                      .trim()
                      .replace(/^\/+/, "")
                      .replace(/\//g, "-")}`
                  : "<md5(cwd)>"}
                /{session.uuid}/
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
                Layer 3 (state.vscdb composerData)
              </div>
              <div className="text-xs text-fg-muted mt-0.5">
                L3 由 Cursor Desktop 自己管理, 强制删除需要 cursaves 的
                staged-copy 路径, v0.2.1 暂不支持.
              </div>
            </div>
          </label>

          {deleteReport?.cursor_running && (
            <div className="text-xs text-accent-red bg-accent-red/10 border border-accent-red/30 rounded px-2 py-1.5">
              检测到 Cursor / cursor-agent 在跑 (pid 数:{" "}
              {deleteReport.running_processes.length}).
              请关闭后重试, "确认删除"按钮已禁用.
            </div>
          )}
          {deleteReport && !deleteReport.cursor_running && (
            <div className="text-xs text-fg-secondary bg-bg-tertiary border border-border rounded px-2 py-1.5">
              <div>
                L1:{" "}
                {deleteReport.removed_l1
                  ? "✓ 已删"
                  : `跳过 (${deleteReport.skipped_l1 ?? "未知"})`}
              </div>
              <div>
                L2:{" "}
                {deleteReport.removed_l2
                  ? "✓ 已删"
                  : `跳过 (${deleteReport.skipped_l2 ?? "未知"})`}
              </div>
            </div>
          )}
          {deleteError && (
            <div className="text-xs text-accent-red">错误: {deleteError}</div>
          )}
        </div>
        <div className="px-5 py-3 border-t border-border flex items-center justify-end gap-2">
          <button
            type="button"
            onClick={closeDeleteDialog}
            disabled={deleteRunning}
            className="px-3 py-1.5 rounded-md bg-bg-tertiary border border-border text-xs hover:bg-bg-hover disabled:opacity-50"
          >
            取消
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
                删除中…
              </>
            ) : (
              <>
                <Trash2 size={11} />
                确认删除
              </>
            )}
          </button>
        </div>
      </dialog>
    </div>
  );
}
