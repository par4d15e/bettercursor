// src/components/BrokenBadge.tsx — small inline marker for broken sessions.
//
// Renders a yellow `⚠` chip with the reason as a tooltip. Used in:
//   - SessionTree row (compact, single-char) — title attribute carries full text
//   - SessionDetail metadata banner (full text visible)
//
// Backend currently flags a session as broken iff Layer 2 store.db has
// `latestRootBlobId == ""` (a known cursor-agent data-loss mode that
// `cursor-agent --resume <uuid>` will reject). v0.2 will add a "修复"
// button; for v0.1 we just surface the issue so users stop wasting
// time on impossible-to-resume sessions.

import { AlertTriangle } from "lucide-react";

interface Props {
  reason?: string;
  size?: "sm" | "md";
}

export function BrokenBadge({ reason, size = "sm" }: Props) {
  const cls =
    size === "sm"
      ? "text-[10px] px-1 py-0.5"
      : "text-xs px-2 py-1";
  return (
    <span
      className={`inline-flex items-center gap-1 rounded font-medium bg-accent-yellow/15 border border-accent-yellow/40 text-accent-yellow ${cls}`}
      title={reason ?? "数据残缺"}
    >
      <AlertTriangle size={size === "sm" ? 10 : 12} />
      {size === "md" && <span>{reason ?? "数据残缺"}</span>}
    </span>
  );
}
