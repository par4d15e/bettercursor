// src/components/BubbleView.tsx — one chat bubble (user or assistant).
//
// Markdown rendering: tiny inline-only parser (~120 lines, no deps).
// Supports block elements (# ## ### headings, - * 1. lists, paragraphs)
// and inline markers (`code`, **bold**, *italic*, [text](url)).
// Code blocks already split out below. Everything else goes through
// the block → inline pipeline.
//
// Anti-XSS: HTML is escaped BEFORE any markdown replacement runs, and
// the only HTML we ever produce comes from our own interpolation
// of previously-escaped text. We do not pass user content verbatim
// into dangerouslySetInnerHTML.

import type { Bubble } from "../lib/tauri";
import { AlertCircle } from "lucide-react";

type Block =
  | { kind: "code"; body: string }
  | { kind: "h1" | "h2" | "h3"; text: string }
  | { kind: "ul" | "ol"; items: string[] }
  | { kind: "p"; text: string };

interface MarkdownPart {
  kind: "code" | "text";
  body: string;
}

/// Split raw bubble text into (code|text) parts on triple-backtick fences.
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
    const close = text.indexOf("```", fence + 3);
    if (close === -1) {
      parts.push({ kind: "text", body: text.slice(fence) });
      break;
    }
    parts.push({ kind: "code", body: text.slice(fence + 3, close) });
    i = close + 3;
  }
  return parts;
}

/// Tokenize a non-code text section into block elements.
function parseBlocks(text: string): Block[] {
  const lines = text.split("\n");
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const ln = lines[i];
    if (ln.trim() === "") {
      i++;
      continue;
    }
    let m: RegExpMatchArray | null;
    if ((m = ln.match(/^###\s+(.*)$/))) {
      blocks.push({ kind: "h3", text: m[1] });
      i++;
    } else if ((m = ln.match(/^##\s+(.*)$/))) {
      blocks.push({ kind: "h2", text: m[1] });
      i++;
    } else if ((m = ln.match(/^#\s+(.*)$/))) {
      blocks.push({ kind: "h1", text: m[1] });
      i++;
    } else if (/^[-*]\s+/.test(ln)) {
      const items: string[] = [];
      while (i < lines.length && /^[-*]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^[-*]\s+/, ""));
        i++;
      }
      blocks.push({ kind: "ul", items });
    } else if (/^\d+\.\s+/.test(ln)) {
      const items: string[] = [];
      while (i < lines.length && /^\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\d+\.\s+/, ""));
        i++;
      }
      blocks.push({ kind: "ol", items });
    } else {
      const buf: string[] = [ln];
      i++;
      while (
        i < lines.length &&
        lines[i].trim() !== "" &&
        !/^#{1,3}\s/.test(lines[i]) &&
        !/^[-*]\s/.test(lines[i]) &&
        !/^\d+\.\s/.test(lines[i])
      ) {
        buf.push(lines[i]);
        i++;
      }
      blocks.push({ kind: "p", text: buf.join("\n") });
    }
  }
  return blocks;
}

// ── Inline renderer (markdown → React nodes) ─────────────────────

const ESCAPE_MAP: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ESCAPE_MAP[c]);
}

/// Reject javascript:/data:/vbscript: URIs and strip whitespace. Anything
/// else (http, https, mailto, relative, fragment) is passed through.
function sanitizeUrl(s: string): string {
  const t = s.trim();
  if (/^(javascript|data|vbscript):/i.test(t)) return "about:blank";
  return t;
}

/// Render inline markdown (in an already-HTML-escaped string) into React
/// nodes. All five markers (`code`, **bold**, *italic*, [text](url),
/// newlines for soft breaks) are supported.
function renderInline(escaped: string): React.ReactNode {
  // Single-pass tokenizer: find the EARLIEST match among the supported
  // inline markers, emit literal text up to it, then a styled node.
  const nodes: React.ReactNode[] = [];
  let rest = escaped;
  let key = 0;

  const PATTERNS: Array<{
    re: RegExp;
    toNode: (m: RegExpMatchArray) => React.ReactNode;
  }> = [
    {
      re: /`([^`]+)`/,
      toNode: (m) => (
        <code
          key={key++}
          className="text-xs font-mono px-1 py-0.5 rounded bg-bg-primary border border-border text-accent-green"
        >
          {m[1]}
        </code>
      ),
    },
    {
      re: /\*\*([^*\n][^*]*?)\*\*/,
      toNode: (m) => <strong key={key++}>{m[1]}</strong>,
    },
    {
      re: /(?<!\*)\*(?!\*)([^*\n][^*]*?)\*(?!\*)/,
      toNode: (m) => <em key={key++}>{m[1]}</em>,
    },
    {
      re: /\[([^\]]+)\]\(([^)]+)\)/,
      toNode: (m) => {
        const href = sanitizeUrl(m[2]);
        return (
          <a
            key={key++}
            href={href}
            target="_blank"
            rel="noreferrer"
            className="text-accent-blue underline hover:text-accent-blue/80"
          >
            {m[1]}
          </a>
        );
      },
    },
  ];

  while (rest.length > 0) {
    let earliestIdx = -1;
    let earliestMatch: RegExpMatchArray | null = null;
    let earliestPat: number = -1;
    for (let p = 0; p < PATTERNS.length; p++) {
      // Need to re-search after each replacement because indices shift.
      // Easier: search in rest directly. Use non-stateful regex.
      PATTERNS[p].re.lastIndex = 0;
      const m = rest.match(PATTERNS[p].re);
      if (m && m.index !== undefined) {
        if (earliestIdx === -1 || m.index < earliestIdx) {
          earliestIdx = m.index;
          earliestMatch = m;
          earliestPat = p;
        }
      }
    }
    if (!earliestMatch || earliestIdx === -1) {
      // Emit the remaining literal text, with `\n` → <br/> soft breaks.
      const lit = rest;
      const segs = lit.split("\n");
      nodes.push(
        ...segs.map((s, idx) => (
          <span key={key++}>
            {s}
            {idx < segs.length - 1 && <br />}
          </span>
        )),
      );
      break;
    }
    // Emit text before the earliest match.
    if (earliestIdx > 0) {
      const prefix = rest.slice(0, earliestIdx);
      const segs = prefix.split("\n");
      nodes.push(
        ...segs.map((s, idx) => (
          <span key={key++}>
            {s}
            {idx < segs.length - 1 && <br />}
          </span>
        )),
      );
    }
    nodes.push(PATTERNS[earliestPat].toNode(earliestMatch));
    rest = rest.slice(earliestIdx + earliestMatch[0].length);
  }

  return nodes;
}

function renderBlock(block: Block, keyPrefix: string) {
  const esc = (s: string) => renderInline(escapeHtml(s));
  switch (block.kind) {
    case "code":
      return (
        <pre
          key={keyPrefix}
          className="text-xs font-mono px-2 py-1.5 rounded bg-bg-primary border border-border overflow-x-auto whitespace-pre"
        >
          <code>{block.body}</code>
        </pre>
      );
    case "h1":
      return (
        <h1
          key={keyPrefix}
          className="text-base font-bold text-fg-primary mt-2 mb-1"
        >
          {esc(block.text)}
        </h1>
      );
    case "h2":
      return (
        <h2
          key={keyPrefix}
          className="text-sm font-bold text-fg-primary mt-2 mb-1"
        >
          {esc(block.text)}
        </h2>
      );
    case "h3":
      return (
        <h3
          key={keyPrefix}
          className="text-sm font-semibold text-fg-primary mt-1.5 mb-0.5"
        >
          {esc(block.text)}
        </h3>
      );
    case "ul":
      return (
        <ul
          key={keyPrefix}
          className="text-sm leading-relaxed list-disc list-inside space-y-0.5"
        >
          {block.items.map((it, idx) => (
            <li key={idx}>{esc(it)}</li>
          ))}
        </ul>
      );
    case "ol":
      return (
        <ol
          key={keyPrefix}
          className="text-sm leading-relaxed list-decimal list-inside space-y-0.5"
        >
          {block.items.map((it, idx) => (
            <li key={idx}>{esc(it)}</li>
          ))}
        </ol>
      );
    case "p":
      return (
        <p
          key={keyPrefix}
          className="text-sm leading-relaxed whitespace-pre-wrap"
        >
          {esc(block.text)}
        </p>
      );
  }
}

export function BubbleView({ bubble }: { bubble: Bubble }) {
  const isUser = bubble.role === "user";
  const isTool =
    bubble.role !== "user" &&
    bubble.role !== "assistant" &&
    bubble.role.length > 0;
  const parts = bubble.text.length > 0 ? splitCodeFences(bubble.text) : [];

  return (
    <div
      className={`flex ${isUser ? "justify-end" : "justify-start"} mb-3`}
    >
      <div
        className={`max-w-[85%] rounded-lg px-3 py-2 ${
          isUser
            ? "bg-accent-blue/15 border border-accent-blue/30 text-fg-primary"
            : isTool
              ? "bg-bg-tertiary border border-border text-fg-muted italic"
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

        {/* Body — markdown renderer */}
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
                <div key={idx} className="space-y-1.5">
                  {parseBlocks(p.body).map((b, bidx) => renderBlock(b, `${idx}-${bidx}`))}
                </div>
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
                className="text-[10px] font-mono px-2 py-1 rounded bg-bg-primary border border-border text-fg-secondary flex items-center gap-1.5"
              >
                <AlertCircle size={10} className="text-accent-green shrink-0" />
                <span className="font-semibold text-fg-primary">{tc.name}</span>
                {tc.input != null && (
                  <span className="text-fg-muted truncate">
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
