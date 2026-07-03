// src/components/SourceBadge.tsx — small colored chip per source layer

import type { SourceLayer } from "../lib/types";
import { SOURCE_LABELS, SOURCE_COLORS } from "../lib/types";

interface SourceBadgeProps {
  source: SourceLayer;
  size?: "sm" | "md";
}

export function SourceBadge({ source, size = "sm" }: SourceBadgeProps) {
  return (
    <span
      className={`inline-flex items-center rounded-md border font-mono ${
        SOURCE_COLORS[source]
      } ${size === "sm" ? "px-1.5 py-0.5 text-[10px]" : "px-2 py-1 text-xs"}`}
      title={SOURCE_LABELS[source]}
    >
      {SOURCE_LABELS[source]}
    </span>
  );
}
