// src/components/MessageList.tsx — right-panel conversation list.
//
// v0.2.2 thin wrapper around <BubbleView>. Single responsibility:
//   1. Sticky header showing bubble count + parse_errors count.
//   2. Three-state copy: loading / error / empty.
//   3. Floating "jump to bottom" button when the user has scrolled
//      up past 200px from the bottom.
//   4. Forwards bubble.id as the React key (was `idx` in SessionDetail,
//      which forces a full re-render every time the list mutates —
//      visible as visible flash in the right panel after each
//      sync_session_layer23 run).
//
// Design constraints (v0.2.2 scope):
//   - No virtual scroll (deferred to v0.2.3 with "large session" support).
//   - No markdown upgrade (BubbleView's self-rolled parser is
//     intentionally kept at 0 deps).
//   - No folding, search, toolbar (those are v0.2.3+).
//
// Props mirror what SessionDetail already collected internally
// (`conv` + `loading` + `error`), so the call-site change is one
// JSX line — see SessionDetail.tsx lines 482-523 for the diff.

import { useEffect, useRef, useState } from "react";
import { ArrowDown } from "lucide-react";
import { BubbleView } from "./BubbleView";
import type { Conversation } from "../lib/tauri";

export interface MessageListProps {
  conv: Conversation | null;
  loading: boolean;
  error: string | null;
}

/// Distance from the bottom (in px) within which we treat the user as
/// "at the bottom" — used both to decide whether the floating button
/// appears and whether a new bubble arrival should auto-scroll.
const JUMP_THRESHOLD_PX = 200;

export function MessageList({ conv, loading, error }: MessageListProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [showJump, setShowJump] = useState(false);

  // Auto-scroll on new bubbles when the user is already near the
  // bottom. If they're reading older messages we leave the position
  // alone — the floating button takes over so they can come back down.
  useEffect(() => {
    if (!conv) return;
    const el = scrollRef.current;
    if (!el) return;
    const distFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    if (distFromBottom < JUMP_THRESHOLD_PX) {
      el.scrollTop = el.scrollHeight;
    }
  }, [conv?.bubbles.length]);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const distFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    setShowJump(distFromBottom > JUMP_THRESHOLD_PX);
  };

  const jumpToBottom = () => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  };

  return (
    <div className="flex-1 flex flex-col overflow-hidden relative">
      {/* Sticky header */}
      <div className="sticky top-0 z-10 px-6 py-2 bg-bg-primary border-b border-border flex items-center gap-2">
        <h3 className="text-xs font-semibold text-fg-secondary">对话记录</h3>
        {conv && (
          <span className="text-xs text-fg-muted font-mono">
            ({conv.bubbles.length}
            {conv.parse_errors > 0 &&
              `, ${conv.parse_errors} 行解析失败`}
            )
          </span>
        )}
      </div>

      {/* Scrollable content */}
      <div
        ref={scrollRef}
        onScroll={handleScroll}
        className="flex-1 overflow-y-auto px-6 py-4"
      >
        {loading && (
          <div className="text-xs text-fg-muted italic">加载中…</div>
        )}

        {error && (
          <div className="text-xs text-accent-red">加载失败: {error}</div>
        )}

        {!loading && !error && conv && conv.bubbles.length === 0 && (
          <div className="text-xs text-fg-muted italic">
            {conv.source_path
              ? "该会话的 JSONL 已找到, 但没有可解析的对话气泡 (可能为空会话)."
              : "该会话在 Layer 1 JSONL 中未找到. 仅 Layer 2/3 来源, 对话内容暂不可用."}
          </div>
        )}

        {!loading && !error && conv && conv.bubbles.length > 0 && (
          <div>
            {conv.bubbles.map((bubble, idx) => (
              <BubbleView
                key={bubble.id || `idx-${idx}`}
                bubble={bubble}
              />
            ))}
          </div>
        )}
      </div>

      {/* Floating jump-to-bottom button */}
      {showJump && (
        <button
          type="button"
          data-testid="jump-to-bottom"
          onClick={jumpToBottom}
          className="absolute bottom-4 right-4 z-20 p-2 rounded-full bg-bg-tertiary border border-border text-fg-secondary hover:text-fg-primary hover:bg-bg-hover shadow-lg"
          title="跳到底部"
        >
          <ArrowDown size={14} />
        </button>
      )}
    </div>
  );
}