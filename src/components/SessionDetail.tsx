// src/components/SessionDetail.tsx — right panel: title + metadata + resume cmd

import { useEffect, useMemo, useState } from "react";
import { useSessionStore } from "../store/sessionStore";
import { SourceBadge } from "./SourceBadge";
import { BubbleView } from "./BubbleView";
import {
  getConversation,
  getResumeCommand,
  dryRunInjectLayer3,
  commitInjectLayer3,
  refreshSessions,
  type Conversation,
  type InjectPlan,
  type InjectResult,
} from "../lib/tauri";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Copy, Trash2, Folder, Clock, FileText, Hash, ArrowDownToLine, AlertCircle } from "lucide-react";
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
  // Layer 3 injection: only the dry-run preview is held in component
  // state; on confirm the plan is replayed verbatim to commit_inject_layer3.
  const [injectPlan, setInjectPlan] = useState<InjectPlan | null>(null);
  const [injectLoading, setInjectLoading] = useState(false);
  const [injectError, setInjectError] = useState<string | null>(null);
  const [injectResult, setInjectResult] = useState<InjectResult | null>(null);

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
    setInjectError(null);
    setInjectResult(null);
    setInjectLoading(false);
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

  // Layer 3 injection: two-phase API.
  //   Phase 1: dryRun → fill `injectPlan` (state on the page).
  //   Phase 2: commit(plan) → fill `injectResult`.
  // Each phase is gated by `injectLoading` so we don't accept double-clicks.
  const handleDryRunInject = async () => {
    if (!session) return;
    setInjectLoading(true);
    setInjectError(null);
    setInjectResult(null);
    try {
      const plan = await dryRunInjectLayer3(session.uuid);
      setInjectPlan(plan);
    } catch (e) {
      setInjectError(e instanceof Error ? e.message : String(e));
    } finally {
      setInjectLoading(false);
    }
  };

  const handleCommitInject = async () => {
    if (!injectPlan || !session) return;
    setInjectLoading(true);
    setInjectError(null);
    try {
      const r = await commitInjectLayer3(injectPlan);
      setInjectResult(r);
      // After successful commit, refresh from the backend so the
      // Sidebar row's "desktop visibility" reflects reality without
      // waiting for the next watcher tick.
      const count = await refreshSessions();
      const fresh = useSessionStore.getState().sessions;
      console.log(`[bettercursor] post-inject refresh: ${fresh.length}/${count}`);
    } catch (e) {
      setInjectError(e instanceof Error ? e.message : String(e));
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
            Three phases: idle (offer button) → dry-run preview → done.
            Cursor Electron must be restarted for Sidebar to show the
            new entry; we surface that constraint explicitly. */}
        {canInjectLayer3 && (
          <div className="mb-3 px-3 py-2 rounded-md bg-accent-blue/10 border border-accent-blue/40 text-fg-primary text-xs flex items-start gap-2">
            <ArrowDownToLine size={14} className="text-accent-blue shrink-0 mt-px" />
            <div className="flex-1">
              <div className="font-semibold text-accent-blue">
                Desktop Sidebar 看不到此 session
              </div>
              <div className="text-fg-secondary mt-0.5">
                它由 <span className="font-mono">cursor-agent</span> 写入 (Layer 2),
                但没有对应的 Layer 3 entry. 点击下方按钮合成一份注入 Cursor
                Electron 的 state.vscdb. <span className="text-fg-muted">完成后需
                重启 Cursor 才能在 Sidebar 看到.</span>
              </div>

              {/* Phase 1: idle */}
              {!injectPlan && !injectResult && (
                <button
                  onClick={handleDryRunInject}
                  disabled={injectLoading}
                  className="mt-2 px-3 py-1 rounded bg-accent-blue/20 border border-accent-blue/40 text-accent-blue text-xs font-medium hover:bg-accent-blue/30 disabled:opacity-50"
                >
                  {injectLoading ? "扫描中…" : "预览注入计划"}
                </button>
              )}

              {/* Phase 2: dry-run preview */}
              {injectPlan && !injectResult && (
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
                          onClick={handleCommitInject}
                          disabled={injectLoading}
                          className="px-2.5 py-1 rounded bg-accent-green/20 border border-accent-green/40 text-accent-green text-xs font-medium hover:bg-accent-green/30 disabled:opacity-50"
                        >
                          {injectLoading ? "写入中…" : "确认并写入"}
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

              {/* Phase 3: done */}
              {injectResult && (
                <div className="mt-2 px-2 py-1.5 rounded bg-accent-green/10 border border-accent-green/40 text-accent-green text-[11px]">
                  ✓ 已写入 {injectResult.applied} 条变更. 备份在{" "}
                  <span className="font-mono">{injectResult.backup_path}</span>.
                  完整性检查: {injectResult.integrity_ok ? "ok" : "FAILED"}.
                  <div className="mt-1 text-fg-secondary">
                    重启 Cursor Electron 让 Sidebar 显示此 session.
                  </div>
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
