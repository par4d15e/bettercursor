// src/components/BrokenBadge.test.tsx — lock the v0.2.5 i18n-aware
// fallback behavior of <BrokenBadge>.
//
// Three things to assert:
//   1. When `reason` is omitted, the title attribute is the active
//      locale's `broken.label` (zh-CN: "数据残缺", en: "Incomplete
//      data").
//   2. When `reason` is provided, it overrides the i18n fallback.
//   3. The default size ("sm") still renders the visible chip —
//      covered implicitly by `getByTitle` finding the element.
//
// We don't assert the visible text on the md size here — the only
// observable delta is the title attribute, which is the same in
// both sizes (size only affects what shows up in the chip body).

import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import i18n from "../i18n";
import { BrokenBadge } from "./BrokenBadge";

// The i18n singleton is initialized once on module load and
// remembers the active language. Test order would otherwise leak
// state between cases, so we reset to a known locale before each.
async function setLocale(locale: "zh-CN" | "en") {
  await act(async () => {
    await i18n.changeLanguage(locale);
  });
}

function mount(reason?: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <BrokenBadge reason={reason} />
    </I18nextProvider>,
  );
}

describe("<BrokenBadge> (i18n fallback)", () => {
  beforeEach(async () => {
    await setLocale("zh-CN");
  });

  it("renders zh-CN '数据残缺' fallback when no reason", async () => {
    await setLocale("zh-CN");
    mount();
    expect(screen.getByTitle("数据残缺")).toBeInTheDocument();
  });

  it("renders en 'Incomplete data' fallback when no reason", async () => {
    await setLocale("en");
    mount();
    expect(screen.getByTitle("Incomplete data")).toBeInTheDocument();
  });

  it("uses provided reason over i18n fallback", async () => {
    await setLocale("en");
    mount("store.db latestRootBlobId is empty");
    expect(
      screen.getByTitle("store.db latestRootBlobId is empty"),
    ).toBeInTheDocument();
    // The fallback should NOT be present in the DOM when reason wins.
    expect(screen.queryByTitle("Incomplete data")).toBeNull();
  });
});
