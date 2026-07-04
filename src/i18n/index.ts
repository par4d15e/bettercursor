// src/i18n/index.ts — i18next init + export Locale utilities.
//
// Side-effect import: `./main.tsx` does `import "./i18n"` once at app
// boot. After the `void i18n.init(...)` chain resolves, all components
// can `useTranslation()` straight away — we don't block the first
// render on translation load (resources are inline-embedded JSON).
//
// Detection order:
//   1. localStorage[`i18nextLng`]   (set by `changeLanguage`, persists
//      across launches)
//   2. navigator.language          (browser default; Tauri runs inside
//      WebView, so this is the host OS locale)
//
// Fallback: zh-CN (matches the codebase's authoring language).
//
// v0.2.5: only zh-CN + en. Adding a new locale = drop a JSON file in
// `src/locales/`, append the code to SUPPORTED_LOCALES + resources.

import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";
import zhCN from "../locales/zh-CN.json";
import en from "../locales/en.json";

export const SUPPORTED_LOCALES = ["zh-CN", "en"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      "zh-CN": { translation: zhCN },
      en: { translation: en },
    },
    fallbackLng: "zh-CN",
    supportedLngs: [...SUPPORTED_LOCALES],
    interpolation: {
      // React already escapes interpolation — i18next's default
      // double-escapes for non-React consumers, which would mangle
      // code samples like `cursor-agent --resume`.
      escapeValue: false,
    },
    detection: {
      order: ["localStorage", "navigator"],
      caches: ["localStorage"],
      lookupLocalStorage: "i18nextLng",
    },
  });

export default i18n;
