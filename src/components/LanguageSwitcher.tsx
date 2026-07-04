// src/components/LanguageSwitcher.tsx — header dropdown for switching
// the active i18next language.
//
// v0.2.5: simple native <select> so we don't pull in a dropdown menu
// library just for one picker. The selected value is sourced from
// `i18n.language` (the canonical i18next ref) and persists across
// reloads via the `localStorage` cache configured in `src/i18n/index.ts`.
//
// Switching is synchronous: i18next emits `languageChanged` and every
// `useTranslation()` consumer re-renders with the new strings. No
// re-mount of the React tree, no flash of stale text.

import { useTranslation } from "react-i18next";
import { SUPPORTED_LOCALES, type Locale } from "../i18n";

export function LanguageSwitcher() {
  const { i18n } = useTranslation();
  // Native <select> dispatches onChange with the new value already as
  // the right Locale type — cast is safe because the <option>s are
  // pinned to SUPPORTED_LOCALES.
  return (
    <select
      data-testid="language-switcher"
      value={i18n.language}
      onChange={(e) => {
        const next = e.target.value as Locale;
        if (SUPPORTED_LOCALES.includes(next)) {
          void i18n.changeLanguage(next);
        }
      }}
      className="bg-bg-tertiary border border-border text-xs text-fg-secondary rounded px-1.5 py-0.5 cursor-pointer hover:text-fg-primary focus:outline-none focus:border-border-strong"
      title="Language"  // hardcoded; same label works for any locale
      aria-label="Language"
    >
      <option value="zh-CN">中文</option>
      <option value="en">English</option>
    </select>
  );
}
