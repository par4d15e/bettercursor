// vitest.config.ts — v0.2.6 housekeeping: plug vitest 2 into the
// Vite 7 + React 19 toolchain. Mirrors the @vitejs/plugin-react
// chain from vite.config.ts so component tests resolve JSX/TS
// identically to the production build. jsdom (not happy-dom) is
// the default per vitest docs and is what's been battle-tested
// with React 19 + @testing-library/react 16.

/// <reference types="vitest" />
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    // Don't try to parse CSS modules from .css imports during
    // component tests — we don't have a Tailwind transformer wired
    // into vitest, and component tests don't need pixel-accurate
    // CSS resolution to assert on tooltips/text.
    css: false,
    coverage: {
      reporter: ["text", "html"],
      exclude: ["src/test/**", "src/**/*.test.{ts,tsx}"],
    },
  },
});
