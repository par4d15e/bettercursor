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
// v0.2.5: i18n — sticky-header title, count badges, empty-state,
// loading/error copy, jump-to-bottom tooltip all go through `t`.

import { useEffect, useRef, useState } from "react";
import { ArrowDown } from "lucide-react";
import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation();

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
        <h3 className="text-xs font-semibold text-fg-secondary">
          {t("message.title")}
        </h3>
        {conv && (
          <span className="text-xs text-fg-muted font-mono">
            {conv.parse_errors > 0
              ? t("message.countWithErrors", {
                  count: conv.bubbles.length,
                  errors: conv.parse_errors,
                })
              : t("message.count", { count: conv.bubbles.length })}
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
          <div className="text-xs text-fg-muted italic">{t("message.loading")}</div>
        )}

        {error && (
          <div className="text-xs text-accent-red">
            {t("message.loadFailed", { msg: error })}
          </div>
        )}

        {!loading && !error && conv && conv.bubbles.length === 0 && (
          <div className="text-xs text-fg-muted italic">
            {conv.source_path
              ? t("message.emptyWithPath")
              : t("message.emptyNoPath")}
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
          title={t("message.jumpToBottomTitle")}
        >
          <ArrowDown size={14} />
        </button>
      )}
    </div>
  );
}
