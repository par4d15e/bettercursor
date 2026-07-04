// src/components/SourceBadge.tsx — small colored chip per source layer.
//
// v0.2.5: i18n — labels moved out of `lib/types.ts`'s static
// `SOURCE_LABELS` map and into `useTranslation().t("source.*")`,
// so the chip text follows the active locale. `SOURCE_COLORS`
// (Tailwind classes — platform-agnostic) stays in `lib/types.ts`.

import { useTranslation } from "react-i18next";
import type { SourceLayer } from "../lib/types";
import { SOURCE_COLORS } from "../lib/types";

interface SourceBadgeProps {
  source: SourceLayer;
  size?: "sm" | "md";
}

export function SourceBadge({ source, size = "sm" }: SourceBadgeProps) {
  const { t } = useTranslation();
  const label = t(`source.${source}`);
  return (
    <span
      className={`inline-flex items-center rounded-md border font-mono ${
        SOURCE_COLORS[source]
      } ${size === "sm" ? "px-1.5 py-0.5 text-[10px]" : "px-2 py-1 text-xs"}`}
      title={label}
    >
      {label}
    </span>
  );
}
