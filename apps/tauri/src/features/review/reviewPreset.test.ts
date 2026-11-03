import { describe, expect, it } from "vitest";
import type { ProviderInfo } from "../../contracts/runtime";
import type { ReviewPreset, ReviewSettings } from "../../contracts/workbench";
import {
  initialPreset,
  preferredReviewer,
  presetKey,
  readyToReview,
  reviewHint,
  reviewers,
} from "./reviewPreset";

const standard: ReviewPreset = {
  id: "standard",
  name: "Review the change",
  detail: "Correctness, missing tests, and anything that would block a merge.",
  tier: null,
};
const custom: ReviewPreset = { id: "custom", name: "Custom", detail: "", tier: null };

const settings: ReviewSettings = {
  default_preset: "deep",
  presets: [standard, { id: "deep", name: "Deep review", detail: "", tier: "apex" }, custom],
  rules: [],
  errors: [],
};

const installed = { status: { Installed: { version: "1.0" } } } as ProviderInfo["detection"];

function provider(
  id: string,
  structured: boolean,
  mode: string | null = "plan mode",
): ProviderInfo {
  return {
    descriptor: {
      id,
      display_name: id.toUpperCase(),
      capabilities: { supports_initial_prompt: true },
      review: mode ? { args: [], label: mode } : null,
      structured_review: structured
        ? { session_id_flag: "--session-id", add_dir_flag: "--add-dir" }
        : null,
    },
    detection: installed,
  };
}

describe("reviewPreset", () => {
  it("keys the remembered preset per project", () => {
    expect(presetKey("p1")).toBe("ui.review.preset.p1");
  });

  it("starts from the remembered preset when it is still listed", () => {
    expect(initialPreset(settings, "standard")).toBe("standard");
  });

  it("falls back to the default for an unlisted or missing remembered preset", () => {
    expect(initialPreset(settings, "estandar")).toBe("deep");
    expect(initialPreset(settings, undefined)).toBe("deep");
  });

  it("offers only installed providers that can answer a structured review", () => {
    const missing: ProviderInfo = { ...provider("gone", true), detection: undefined };
    const list = reviewers([
      provider("codex", false),
      provider("claude", true),
      missing,
      provider("noplan", true, null),
    ]);
    expect(list).toEqual([{ id: "claude", name: "CLAUDE", mode: "plan mode" }]);
  });

  it("keeps a remembered reviewer only while it is offered", () => {
    const list = reviewers([provider("claude", true), provider("other", true)]);
    expect(preferredReviewer(list, "other")?.id).toBe("other");
    expect(preferredReviewer(list, "gone")?.id).toBe("claude");
    expect(preferredReviewer([], "claude")).toBeNull();
  });

  it("waits for wording before a custom review can start", () => {
    const [claude] = reviewers([provider("claude", true)]);
    expect(readyToReview(claude, standard, "")).toBe(true);
    expect(readyToReview(claude, custom, "  ")).toBe(false);
    expect(readyToReview(claude, custom, "Check the refunds path.")).toBe(true);
    expect(readyToReview(null, standard, "")).toBe(false);
    expect(readyToReview(claude, null, "")).toBe(false);
    expect(reviewHint(claude, custom, "")).toMatch(/Write what to ask for/);
    expect(reviewHint(null, standard, "")).toMatch(/No installed agent/);
    expect(reviewHint(claude, standard, "")).toBe(
      "CLAUDE reads the change in plan mode — it cannot write to this checkout.",
    );
  });
});
