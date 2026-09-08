import { describe, expect, it } from "vitest";
import { updatePill } from "./updatePill";

describe("updatePill", () => {
  it("says nothing about a background check that found nothing", () => {
    expect(updatePill({ state: "checking" })).toBeNull();
    expect(updatePill({ state: "uptodate" })).toBeNull();
  });

  // The menu item is the one case where "no news" is the answer the user asked
  // for, so the same state has to produce a pill there and not otherwise.
  it("answers a check the user asked for", () => {
    expect(updatePill({ state: "uptodate" }, true)?.label).toBe("Up to date");
  });

  it("offers a soft update as one click", () => {
    const pill = updatePill({
      state: "available",
      version: "0.2.0",
      current_version: "0.1.0",
      notes: "Tabs reorder now.",
      kind: "soft",
    });
    expect(pill?.actionable).toBe(true);
    expect(pill?.label).toBe("Update to 0.2.0");
  });

  // A protocol bump costs the user their live terminals, so it must never be
  // one click away.
  it("refuses to make a hard update clickable", () => {
    const pill = updatePill({
      state: "available",
      version: "0.3.0",
      current_version: "0.1.0",
      notes: "",
      kind: "hard",
    });
    expect(pill?.actionable).toBe(false);
    expect(pill?.tone).toBe("warn");
  });

  it("reports progress without offering a second click", () => {
    const pill = updatePill({ state: "downloading", percent: 42 });
    expect(pill?.label).toBe("Downloading 42%");
    expect(pill?.actionable).toBe(false);
  });

  it("keeps a silent failure silent unless the user started it", () => {
    expect(updatePill({ state: "failed", message: "offline" })).toBeNull();
    expect(updatePill({ state: "failed", message: "offline" }, true)?.title).toBe("offline");
  });
});
