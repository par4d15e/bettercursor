// src/components/SyncStatusBadge.test.tsx — lock the v0.2.5
// i18n-aware formatAge signature across both supported locales.
//
// Why a unit test on a pure helper instead of a render test?
// formatAge() is the only piece of the badge whose output varies
// with the active locale. The badge itself is a thin
// `useTranslation()` consumer that ticks once per second; mounting
// it would need fake timers + store wiring for marginal value.
// The pure function is the behavior we care about, so we test it
// directly. Run via `pnpm test`.

import { describe, it, expect } from "vitest";
import { formatAge } from "./SyncStatusBadge";
import zhCN from "../locales/zh-CN.json";
import en from "../locales/en.json";

// Minimal t() shim: walks the nested JSON for a dotted key like
// "sync.xSecondsAgo", then runs i18next-style {{n}} interpolation
// (i18next's default format is `{{varName}}` for the v0.2.5 keys
// we exercise here). Adequate for the 5 keys formatAge touches.
type Resources = Record<string, unknown>;
const buildT = (resources: Resources) => {
  const t = (key: string, opts: Record<string, unknown> = {}) => {
    const parts = key.split(".");
    let cur: unknown = resources;
    for (const p of parts) {
      if (cur && typeof cur === "object" && p in (cur as object)) {
        cur = (cur as Record<string, unknown>)[p];
      } else {
        return key;
      }
    }
    if (typeof cur !== "string") return key;
    return cur.replace(/\{\{(\w+)\}\}/g, (_, k) => String(opts[k] ?? ""));
  };
  return t;
};

const t_zh = buildT(zhCN as unknown as Resources);
const t_en = buildT(en as unknown as Resources);

describe("formatAge (zh-CN)", () => {
  it("returns '尚未扫描' when lastScanMs is null", () => {
    expect(formatAge(null, Date.now(), t_zh)).toBe("尚未扫描");
  });

  it.each([0, 1, 30, 59])("returns '%is 前' under 60s (sec=%i)", (sec) => {
    const now = 1_700_000_000_000;
    const last = now - sec * 1000;
    expect(formatAge(last, now, t_zh)).toBe(`${sec}s 前`);
  });

  it("clamps negative deltas to 0s (clock skew safety)", () => {
    const now = 1_700_000_000_000;
    const last = now + 1000; // 1s in the future
    expect(formatAge(last, now, t_zh)).toBe("0s 前");
  });

  it("returns 'Xm 前' for 1-59 min", () => {
    const now = 1_700_000_000_000;
    expect(formatAge(now - 60_000, now, t_zh)).toBe("1m 前");
    expect(formatAge(now - 30 * 60_000, now, t_zh)).toBe("30m 前");
    expect(formatAge(now - 59 * 60_000, now, t_zh)).toBe("59m 前");
  });

  it("returns 'Xh 前' for >= 60 min", () => {
    const now = 1_700_000_000_000;
    expect(formatAge(now - 60 * 60_000, now, t_zh)).toBe("1h 前");
    expect(formatAge(now - 3 * 60 * 60_000, now, t_zh)).toBe("3h 前");
    expect(formatAge(now - 25 * 60 * 60_000, now, t_zh)).toBe("25h 前");
  });
});

describe("formatAge (en)", () => {
  it("returns 'Not yet scanned' when lastScanMs is null", () => {
    expect(formatAge(null, Date.now(), t_en)).toBe("Not yet scanned");
  });

  it("uses 'Xs ago' under 60s", () => {
    const now = 1_700_000_000_000;
    expect(formatAge(now - 12_000, now, t_en)).toBe("12s ago");
  });

  it("uses 'Xm ago' for 1-59 min", () => {
    const now = 1_700_000_000_000;
    expect(formatAge(now - 5 * 60_000, now, t_en)).toBe("5m ago");
  });

  it("uses 'Xh ago' for >= 60 min", () => {
    const now = 1_700_000_000_000;
    expect(formatAge(now - 2 * 60 * 60_000, now, t_en)).toBe("2h ago");
  });
});
