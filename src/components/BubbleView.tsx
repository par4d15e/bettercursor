// src/components/BubbleView.tsx — one chat bubble (user or assistant).
//
// v0.2 rendering: minimal inline markdown — fenced code blocks → <pre>,
// everything else paragraph text. Real markdown lib is a future-iteration
// concern (left as v0.3 task).

import type { Bubble } from "../lib/tauri";

interface MarkdownPart {
  kind: "code" | "text";
  body: string;
}

/// Split a bubble's text on triple-backtick fences into (code|text) parts.
function splitCodeFences(text: string): MarkdownPart[] {
  const parts: MarkdownPart[] = [];
  let i = 0;
  while (i < text.length) {
    const fence = text.indexOf("```", i);
    if (fence === -1) {
      parts.push({ kind: "text", body: text.slice(i) });
      break;
    }
    if (fence > i) {
      parts.push({ kind: "text", body: text.slice(i, fence) });
    }
    const closeFence = text.indexOf("```", fence + 3);
    if (closeFence === -1) {
      // unterminated fence — render rest as text
      parts.push({ kind: "text", body: text.slice(fence) });
      break;
    }
    parts.push({ kind: "code", body: text.slice(fence + 3, closeFence) });
    i = closeFence + 3;
  }
  return parts;
}

/// Split a text part into paragraphs on blank lines.
function splitParagraphs(text: string): string[] {
  return text
    .split(/\n\s*\n/)
    .map((p) => p.trim())
    .filter((p) => p.length > 0);
}

function renderText(body: string): React.ReactNode {
  return splitParagraphs(body).map((para, idx) => (
    <p key={idx} className="text-sm leading-relaxed whitespace-pre-wrap">
      {para}
    </p>
  ));
}

export function BubbleView({ bubble }: { bubble: Bubble }) {
  const isUser = bubble.role === "user";
  const parts = bubble.text.length > 0 ? splitCodeFences(bubble.text) : [];

  return (
    <div
      className={`flex ${isUser ? "justify-end" : "justify-start"} mb-3`}
    >
      <div
        className={`max-w-[85%] rounded-lg px-3 py-2 ${
          isUser
            ? "bg-accent-blue/15 border border-accent-blue/30 text-fg-primary"
            : "bg-bg-tertiary border border-border text-fg-primary"
        }`}
      >
        {/* Role label */}
        <div
          className={`text-[10px] uppercase tracking-wide font-semibold mb-1 ${
            isUser ? "text-accent-blue" : "text-fg-muted"
          }`}
        >
          {isUser ? "You" : bubble.role || "Assistant"}
        </div>

        {/* Body — code blocks vs paragraphs */}
        {parts.length === 0 && bubble.text.length === 0 ? null : (
          <div className="space-y-2">
            {parts.map((p, idx) =>
              p.kind === "code" ? (
                <pre
                  key={idx}
                  className="text-xs font-mono px-2 py-1.5 rounded bg-bg-primary border border-border overflow-x-auto whitespace-pre"
                >
                  <code>{p.body}</code>
                </pre>
              ) : (
                <div key={idx}>{renderText(p.body)}</div>
              ),
            )}
          </div>
        )}

        {/* Tool calls */}
        {bubble.tool_calls.length > 0 && (
          <div className="mt-2 space-y-1">
            {bubble.tool_calls.map((tc, idx) => (
              <div
                key={idx}
                className="text-[10px] font-mono px-2 py-1 rounded bg-bg-primary border border-border text-fg-secondary"
              >
                <span className="text-accent-green">↳</span>{" "}
                <span className="font-semibold text-fg-primary">
                  {tc.name}
                </span>
                {tc.input != null && (
                  <span className="text-fg-muted">
                    {" "}
                    {JSON.stringify(tc.input).slice(0, 80)}
                    {JSON.stringify(tc.input).length > 80 ? "…" : ""}
                  </span>
                )}
              </div>
            ))}
          </div>
        )}

        {/* Attachments */}
        {bubble.files.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-1">
            {bubble.files.map((f, idx) => (
              <span
                key={idx}
                className="text-[10px] font-mono px-1.5 py-0.5 rounded bg-bg-primary border border-border text-fg-secondary"
              >
                📎 {f}
              </span>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
