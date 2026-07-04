// src/test/setup.ts — vitest setup file. Two responsibilities:
//
// 1. Register @testing-library/jest-dom matchers (toBeInTheDocument,
//    toHaveTextContent, etc.) so component tests can use idiomatic
//    assertions.
//
// 2. Polyfill `window.matchMedia`. jsdom does not implement it
//    natively; some libs (notably anything Tailwind-related, and
//    some hooks in the watch loop) blow up on the missing API.
//    We don't run a real 1Hz interval in component tests, so a
//    noop is fine.

import "@testing-library/jest-dom/vitest";

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  }),
});
