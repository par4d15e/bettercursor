// src/components/LanguageSwitcher.tsx — header control for switching
// the active i18next language.
//
// v0.2.5: originally a native <select>. Replaced with a segmented
// button group because WebKitGTK renders <select>/<option> with the
// host OS light palette, which clashes with our always-dark theme.
//
// Switching is synchronous: i18next emits `languageChanged` and every
// `useTranslation()` consumer re-renders with the new strings. No
// re-mount of the React tree, no flash of stale text.

import { useTranslation } from "react-i18next";
import { SUPPORTED_LOCALES, type Locale } from "../i18n";

const LOCALE_LABEL_KEYS: Record<Locale, "language.zh-CN" | "language.en"> = {
  "zh-CN": "language.zh-CN",
  en: "language.en",
};

function resolveActiveLocale(language: string): Locale {
  if (SUPPORTED_LOCALES.includes(language as Locale)) {
    return language as Locale;
  }
  if (language.startsWith("zh")) return "zh-CN";
  if (language.startsWith("en")) return "en";
  return "zh-CN";
}

export function LanguageSwitcher({ size = "sm" }: { size?: "sm" | "md" }) {
  const { t, i18n } = useTranslation();
  const active = resolveActiveLocale(i18n.resolvedLanguage ?? i18n.language);
  const textSize = size === "md" ? "text-xs" : "text-[10px]";

  return (
    <div
      data-testid="language-switcher"
      role="group"
      aria-label={t("language.switch")}
      title={t("language.switch")}
      className={`inline-flex items-center rounded border border-border overflow-hidden ${textSize}`}
    >
      {SUPPORTED_LOCALES.map((locale) => {
        const isActive = locale === active;
        return (
          <button
            key={locale}
            type="button"
            data-locale={locale}
            aria-pressed={isActive}
            onClick={() => {
              if (!isActive) void i18n.changeLanguage(locale);
            }}
            className={`px-1.5 py-0.5 transition-colors ${
              isActive
                ? "bg-bg-hover text-fg-primary"
                : "bg-bg-tertiary text-fg-secondary hover:bg-bg-hover hover:text-fg-primary"
            }`}
          >
            {t(LOCALE_LABEL_KEYS[locale])}
          </button>
        );
      })}
    </div>
  );
}
