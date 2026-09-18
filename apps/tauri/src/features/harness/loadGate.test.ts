import { describe, expect, it } from "vitest";
import { shouldLoadFeatureDetail, shouldLoadFeatureList } from "./loadGate";

describe("shouldLoadFeatureList", () => {
  const idle = {
    project: "p",
    loadingList: false,
    listReady: false,
    listError: null as string | null,
  };

  it("loads once for a project with an empty list", () => {
    expect(shouldLoadFeatureList(idle)).toBe(true);
    expect(shouldLoadFeatureList({ ...idle, listReady: true })).toBe(false);
  });

  it("does not loop on error or while a read is in flight", () => {
    expect(shouldLoadFeatureList({ ...idle, loadingList: true })).toBe(false);
    expect(shouldLoadFeatureList({ ...idle, listError: "nope" })).toBe(false);
    expect(shouldLoadFeatureList({ ...idle, project: null })).toBe(false);
  });
});

describe("shouldLoadFeatureDetail", () => {
  const idle = {
    project: "p",
    featureId: 3,
    detailId: null as number | null,
    loadingDetail: false,
    detailError: null as string | null,
  };

  it("loads when this tab has no detail yet", () => {
    expect(shouldLoadFeatureDetail(idle)).toBe(true);
    expect(shouldLoadFeatureDetail({ ...idle, detailId: 3 })).toBe(false);
  });

  it("does not retry a failed read until the error is cleared", () => {
    expect(shouldLoadFeatureDetail({ ...idle, detailError: "gone" })).toBe(false);
    expect(shouldLoadFeatureDetail({ ...idle, loadingDetail: true })).toBe(false);
  });
});
