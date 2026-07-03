// src/components/SessionDetail.tsx — right panel: title + metadata + resume cmd

import { useEffect, useMemo, useState } from "react";
import { useSessionStore } from "../store/sessionStore";
import { SourceBadge } from "./SourceBadge";
import { BubbleView } from "./BubbleView";
import {
  getConversation,
  getResumeCommand,
  dryRunInjectLayer3,
  prepareInjectLayer3,
  inspectPreparedLayer3,
  refreshSessions,
  type Conversation,
  type InjectPlan,
  type PrepareResult,
  type Prepared,
} from "../lib/tauri";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  Copy,
  Trash2,
  Folder,
  Clock,
  FileText,
  Hash,
  Terminal,
  AlertCircle,
  CheckCircle2,
  RefreshCw,
  ClipboardCheck,
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
  // Layer 3 injection (offline two-phase):
  //   Phase 1: dryRun → InjectPlan preview (in-component state).
  //   Phase 2: prepare(plan) → PrepareResult with queue_path +
  //     apply_command. Stored as `prepared`.
  //   Phase 3 (later, manual): user runs `apply_command` after
  //     closing Cursor. On next mount or refresh we re-check via
  //     inspectPreparedLayer3 and surface "已应用" if the marker
  //     sidecar exists.
  const [injectPlan, setInjectPlan] = useState<InjectPlan | null>(null);
  const [prepared, setPrepared] = useState<PrepareResult | null>(null);
  const [appliedProbe, setAppliedProbe] = useState<Prepared | null>(null);
  const [injectLoading, setInjectLoading] = useState(false);
  const [injectError, setInjectError] = useState<string | null>(null);
  const [copyHint, setCopyHint] = useState<string | null>(null);

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

  // Inject button shows iff: this session is CLI-originated (Layer 2 /
  // LinuxCLI is a source) AND Layer 3 has no entry yet (Desktop
  // Sidebar can't see it). When Layer 3 is present OR no Layer 2
  // source exists, hide the button entirely — false negatives are
  // fine (user can hit "Refresh" if they suspect staleness), false
  // positives would let users overwrite a real Desktop entry.
  const canInjectLayer3 = useMemo(() => {
    if (!session) return false;
    if (session.layer_3_present) return false;
    // Need at least one Layer 1 (JSONL) or Layer 2 source to build from.
    return Boolean(
      session.sources.linux_cli || session.sources.linux_desktop || session.sources.mac,
    );
  }, [session]);

  // When switching sessions, drop any prior plan/result so old state
  // doesn't leak into a new selection's UI.
  useEffect(() => {
    setInjectPlan(null);
    setPrepared(null);
    setAppliedProbe(null);
    setInjectError(null);
    setInjectLoading(false);
    setCopyHint(null);
  }, [selectedUuid]);

  // On every selectedUuid change, peek at the queue directory: if
  // the user already prepared (or applied) this uuid in a previous
  // session, surface the marker state so we can render "已应用".
  useEffect(() => {
    if (!selectedUuid) {
      setAppliedProbe(null);
      return;
    }
    let cancelled = false;
    inspectPreparedLayer3(selectedUuid)
      .then((p) => {
        if (!cancelled) setAppliedProbe(p);
      })
      .catch(() => {
        // Don't surface — no queue file just means "not prepared yet".
        if (!cancelled) setAppliedProbe(null);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedUuid]);

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

  // Layer 3 injection: offline two-phase API.
  //   Phase 1: dryRun → fill `injectPlan` (state on the page).
  //   Phase 2: prepare(plan) → fill `prepared` (with apply_command).
  // Phase 3 (apply) is **manual** — done by the user running
  // `apply.py` after closing Cursor Electron. bettercursor
  // intentionally never executes it (see #84 for the WAL race
  // that motivated going offline).
  const handleDryRunInject = async () => {
    if (!session) return;
    setInjectLoading(true);
    setInjectError(null);
    try {
      const plan = await dryRunInjectLayer3(session.uuid);
      setInjectPlan(plan);
    } catch (e) {
      setInjectError(e instanceof Error ? e.message : String(e));
    } finally {
      setInjectLoading(false);
    }
  };

  const handlePrepareInject = async () => {
    if (!session) return;
    setInjectLoading(true);
    setInjectError(null);
    try {
      const result = await prepareInjectLayer3(session.uuid);
      setPrepared(result);
      // Refresh the applied probe — running prepare doesn't set the
      // .applied marker (only apply.py does), but it's good hygiene
      // so a subsequent user click sees fresh state without a remount.
      const fresh = await inspectPreparedLayer3(session.uuid).catch(() => null);
      setAppliedProbe(fresh);
    } catch (e) {
      setInjectError(e instanceof Error ? e.message : String(e));
    } finally {
      setInjectLoading(false);
    }
  };

  const handleCopyApplyCommand = async (cmd: string) => {
    try {
      await writeText(cmd);
      setCopyHint("apply 命令已复制");
      setTimeout(() => setCopyHint(null), 1500);
    } catch (e) {
      console.error("copy apply command failed:", e);
      setCopyHint("复制失败 (见 console)");
    }
  };

  // After a successful prepare, refresh in the background so the
  // Sidebar's "desktop visibility" picks up the moment we read
  // state.vscdb via the next scan — but the actual mutation stays
  // queued until the user runs apply.py. This keeps things honest:
  // we don't pretend the sidebar is updated.
  const handleRecheckApplied = async () => {
    if (!session) return;
    setInjectLoading(true);
    try {
      const fresh = await inspectPreparedLayer3(session.uuid).catch(() => null);
      setAppliedProbe(fresh);
      if (fresh?.applied) {
        // Refresh sessions so the Sidebar reflects the now-
        // actually-applied injection. The user must have restarted
        // Cursor for the rows to be visible there too.
        await refreshSessions();
      }
    } finally {
      setInjectLoading(false);
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
        {/* Broken-state banner (above title so it can't be missed). */}
        {session.is_broken && (
          <div className="mb-3 px-3 py-2 rounded-md bg-accent-yellow/10 border border-accent-yellow/40 text-accent-yellow text-xs flex items-start gap-2">
            <span className="text-base leading-none shrink-0 mt-px">⚠</span>
            <div className="flex-1">
              <div className="font-semibold">
                {session.broken_reason ?? "该 session 数据不完整"}
              </div>
              <div className="text-fg-muted mt-0.5">
                对应 `--resume` 命令可能失败. v0.2 将提供"修复"按钮.
              </div>
            </div>
          </div>
        )}

        {/* Inject-to-Layer-3 banner (CLI session, Desktop Sidebar missing).
            Offline two-phase: dryRun → 预览 injection plan, prepare →
            stage offline apply script for the user to run **after**
            closing Cursor. Why offline? Cursor Electron holds
            state.vscdb open — live writes silently lose rows to its
            WAL flush (see #84). The user must restart Cursor for the
            Sidebar to reflect the change either way. */}
        {canInjectLayer3 && (
          <div className="mb-3 px-3 py-2 rounded-md bg-accent-blue/10 border border-accent-blue/40 text-fg-primary text-xs flex items-start gap-2">
            <Terminal size={14} className="text-accent-blue shrink-0 mt-px" />
            <div className="flex-1">
              <div className="font-semibold text-accent-blue">
                Desktop Sidebar 看不到此 session
              </div>
              <div className="text-fg-secondary mt-0.5">
                它由 <span className="font-mono">cursor-agent</span> 写入 (Layer 2),
                但没有对应的 Layer 3 entry. {" "}
                <span className="text-fg-muted">
                  为防止与 Cursor Electron 的 WAL 写入竞争, bettercursor
                  只生成离线注入包 — 关闭 Cursor 后由你手动
                  </span>
                <span className="font-mono">python3 ~/.bettercursor/apply.py …</span>
                <span className="text-fg-muted"> 完成落地.</span>
              </div>

              {/* Already-applied badge. Wins over all other phases —
                  if the marker sidecar exists, the user already did
                  the manual apply and we just remind them to refresh
                  Cursor if they haven't yet. */}
              {appliedProbe?.applied && (
                <div className="mt-2 px-2 py-1.5 rounded bg-accent-green/10 border border-accent-green/40 text-accent-green text-[11px] flex items-start gap-1.5">
                  <CheckCircle2 size={12} className="shrink-0 mt-px" />
                  <div className="flex-1">
                    <div>
                      ✓ 离线注入包已应用 (marker: <span className="font-mono">{appliedProbe.marker_path}</span>).
                    </div>
                    <div className="mt-0.5 text-fg-secondary">
                      若 Sidebar 还没显示, 请确认已完全退出 Cursor 然后重新打开.
                    </div>
                    <button
                      onClick={handleRecheckApplied}
                      disabled={injectLoading}
                      className="mt-1 px-2 py-0.5 rounded bg-bg-hover border border-border text-fg-secondary text-[11px] hover:bg-bg-primary disabled:opacity-50 inline-flex items-center gap-1"
                    >
                      <RefreshCw size={11} />
                      重新检查 + 刷新 Sidebar
                    </button>
                  </div>
                </div>
              )}

              {/* Phase 0: queue file exists but apply not yet run.
                  We don't show the dry-run preview again — just the
                  copy-able apply command and the option to re-stage. */}
              {!appliedProbe?.applied && prepared && (
                <div className="mt-2 px-2 py-1.5 rounded bg-bg-tertiary border border-border font-mono text-[11px]">
                  <div className="text-fg-secondary">
                    离线注入包已就绪 ({prepared.mutations} 条变更待落地). 关闭
                    Cursor 后运行:
                  </div>
                  <div className="mt-1 flex items-start gap-1">
                    <code className="flex-1 px-1.5 py-1 rounded bg-bg-primary border border-border text-fg-primary break-all whitespace-pre-wrap">
                      {prepared.apply_command}
                    </code>
                    <button
                      onClick={() => handleCopyApplyCommand(prepared.apply_command)}
                      className="p-1 rounded hover:bg-bg-hover text-fg-secondary hover:text-fg-primary shrink-0"
                      title="复制到剪贴板"
                    >
                      <Copy size={12} />
                    </button>
                  </div>
                  <div className="mt-1 text-fg-muted">
                    queue: <span className="text-fg-secondary">{prepared.queue_path}</span>
                  </div>
                  {copyHint && (
                    <div className="mt-1 text-accent-green">✓ {copyHint}</div>
                  )}
                  <div className="mt-2 flex items-center gap-2">
                    <button
                      onClick={handleRecheckApplied}
                      disabled={injectLoading}
                      className="px-2.5 py-1 rounded bg-bg-hover border border-border text-fg-secondary text-xs hover:bg-bg-primary disabled:opacity-50 inline-flex items-center gap-1"
                    >
                      <RefreshCw size={11} />
                      我已运行 apply, 重新检查
                    </button>
                    <button
                      onClick={handlePrepareInject}
                      disabled={injectLoading}
                      className="px-2.5 py-1 rounded bg-bg-hover border border-border text-fg-muted text-xs hover:bg-bg-primary disabled:opacity-50"
                      title="幂等: 重新生成 queue 文件 (apply 未运行时才有效)"
                    >
                      {injectLoading ? "重新生成中…" : "重新生成"}
                    </button>
                  </div>
                </div>
              )}

              {/* Phase 1: idle — surface the dry-run / prepare affordances. */}
              {!appliedProbe?.applied && !prepared && !injectPlan && (
                <button
                  onClick={handleDryRunInject}
                  disabled={injectLoading}
                  className="mt-2 px-3 py-1 rounded bg-accent-blue/20 border border-accent-blue/40 text-accent-blue text-xs font-medium hover:bg-accent-blue/30 disabled:opacity-50"
                >
                  {injectLoading ? "扫描中…" : "预览注入计划"}
                </button>
              )}

              {/* Phase 2: dry-run preview, before staging. */}
              {!appliedProbe?.applied && !prepared && injectPlan && (
                <div className="mt-2 px-2 py-1.5 rounded bg-bg-tertiary border border-border font-mono text-[11px]">
                  {injectPlan.skip_reason ? (
                    <div className="text-accent-red">
                      跳过: {injectPlan.skip_reason}
                    </div>
                  ) : (
                    <>
                      <div className="text-fg-secondary">
                        将写入 {injectPlan.mutations.length} 条变更:
                      </div>
                      <ul className="mt-1 space-y-0.5 text-fg-muted">
                        {injectPlan.mutations.slice(0, 6).map((m, idx) => (
                          <li key={idx}>
                            → <span className="text-fg-primary">{m.key}</span>
                          </li>
                        ))}
                        {injectPlan.mutations.length > 6 && (
                          <li className="text-fg-muted">
                            … 还有 {injectPlan.mutations.length - 6} 条
                          </li>
                        )}
                      </ul>
                      {injectPlan.sources.cwd && (
                        <div className="mt-1 text-fg-muted">
                          workspace: <span className="text-fg-secondary">{injectPlan.sources.cwd}</span>
                        </div>
                      )}
                      <div className="mt-2 flex items-center gap-2">
                        <button
                          onClick={handlePrepareInject}
                          disabled={injectLoading}
                          className="px-2.5 py-1 rounded bg-accent-green/20 border border-accent-green/40 text-accent-green text-xs font-medium hover:bg-accent-green/30 disabled:opacity-50 inline-flex items-center gap-1"
                        >
                          <ClipboardCheck size={11} />
                          {injectLoading ? "准备中…" : "准备离线注入包"}
                        </button>
                        <button
                          onClick={() => setInjectPlan(null)}
                          disabled={injectLoading}
                          className="px-2.5 py-1 rounded bg-bg-hover border border-border text-fg-secondary text-xs hover:bg-bg-primary disabled:opacity-50"
                        >
                          取消
                        </button>
                      </div>
                    </>
                  )}
                </div>
              )}

              {injectError && (
                <div className="mt-2 px-2 py-1.5 rounded bg-accent-red/10 border border-accent-red/40 text-accent-red text-[11px] flex items-start gap-1.5">
                  <AlertCircle size={12} className="shrink-0 mt-px" />
                  <span>{injectError}</span>
                </div>
              )}
            </div>
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
            className="px-3 py-1.5 rounded-md bg-accent-red/15 border border-accent-red/30 text-accent-red text-xs font-medium hover:bg-accent-red/25 disabled:opacity-50"
            disabled
            title="Phase T3: 暂未实现"
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

      {/* Conversation */}
      <div className="flex-1 overflow-y-auto px-6 py-4">
        <div className="flex items-center gap-2 mb-3">
          <h3 className="text-xs font-semibold text-fg-secondary">
            对话记录
          </h3>
          {conv && (
            <span className="text-xs text-fg-muted font-mono">
              ({conv.bubbles.length}
              {conv.parse_errors > 0 &&
                `, ${conv.parse_errors} 行解析失败`}
              )
            </span>
          )}
        </div>

        {convLoading && (
          <div className="text-xs text-fg-muted italic">加载中…</div>
        )}

        {convError && (
          <div className="text-xs text-accent-red">
            加载失败: {convError}
          </div>
        )}

        {!convLoading && conv && conv.bubbles.length === 0 && (
          <div className="text-xs text-fg-muted italic">
            {conv.source_path
              ? "该会话的 JSONL 已找到, 但没有可解析的对话气泡 (可能为空会话)."
              : "该会话在 Layer 1 JSONL 中未找到. 仅 Layer 2/3 来源, 对话内容暂不可用."}
          </div>
        )}

        {!convLoading && conv && conv.bubbles.length > 0 && (
          <div>
            {conv.bubbles.map((bubble, idx) => (
              <BubbleView key={idx} bubble={bubble} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
