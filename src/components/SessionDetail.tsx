// src/components/SessionDetail.tsx — right panel: title + metadata + resume cmd

import { useEffect, useMemo, useState } from "react";
import { useSessionStore } from "../store/sessionStore";
import { SourceBadge } from "./SourceBadge";
import { BubbleView } from "./BubbleView";
import { getConversation, getResumeCommand, type Conversation } from "../lib/tauri";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Copy, Trash2, Folder, Clock, FileText, Hash } from "lucide-react";
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
