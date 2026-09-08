import { describe, expect, it } from "vitest";
import {
  commentSummary,
  fileSummary,
  decisionChip,
  labelColor,
  relationChips,
  splitRepository,
  stateChip,
} from "./prDetail";
import type { PullRequest } from "../runtime/types";

function pr(extra: Partial<PullRequest> = {}): PullRequest {
  return {
    project_id: "p1",
    repository: "acme/widget",
    host: "github.com",
    number: 54,
    title: "Add the diff view",
    body: "",
    body_truncated: false,
    url: "https://example.invalid/54",
    author: "rin",
    base_ref: "main",
    head_ref: "diff-view",
    is_draft: false,
    review_decision: null,
    labels: [],
    assignees: [],
    review_requests: [],
    additions: 412,
    deletions: 118,
    changed_files: 36,
    comment_count: 0,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    relations: { assigned: false, review_requested: false, authored: false },
    ...extra,
  };
}

describe("stateChip", () => {
  it("separates a draft from an open pull request", () => {
    expect(stateChip(pr())).toEqual({ label: "Open", tone: "good" });
    expect(stateChip(pr({ is_draft: true }))).toEqual({ label: "Draft", tone: "neutral" });
  });
});

describe("decisionChip", () => {
  it("tones the verdicts the host names", () => {
    expect(decisionChip("Approved")).toEqual({ label: "Approved", tone: "good" });
    expect(decisionChip("ChangesRequested")).toEqual({
      label: "Changes requested",
      tone: "bad",
    });
    expect(decisionChip("ReviewRequired")).toEqual({ label: "Review required", tone: "warn" });
  });

  it("shows a verdict this build has not heard of rather than dropping it", () => {
    expect(decisionChip("SECOND_OPINION")).toEqual({ label: "Second opinion", tone: "neutral" });
  });

  it("has nothing to say when the host did not decide", () => {
    expect(decisionChip(null)).toBeNull();
    expect(decisionChip("  ")).toBeNull();
  });
});

describe("relationChips", () => {
  it("puts the ask before the rest", () => {
    const chips = relationChips({ assigned: true, review_requested: true, authored: true });
    expect(chips.map((chip) => chip.label)).toEqual([
      "Your review requested",
      "Assigned to you",
      "You opened this",
    ]);
    expect(chips[0].tone).toBe("warn");
  });

  it("is empty for a pull request the viewer is not part of", () => {
    expect(relationChips({ assigned: false, review_requested: false, authored: false })).toEqual(
      [],
    );
  });
});

describe("splitRepository", () => {
  it("splits an owner off, and copes without one", () => {
    expect(splitRepository("acme/widget")).toEqual({ owner: "acme", name: "widget" });
    expect(splitRepository("widget")).toEqual({ owner: null, name: "widget" });
  });
});

describe("labelColor", () => {
  it("takes six hex digits, with or without the hash", () => {
    expect(labelColor("d73a4a")).toBe("#d73a4a");
    expect(labelColor("#D73A4A")).toBe("#D73A4A");
  });

  it("refuses anything that is not a colour", () => {
    // It lands in a `style` attribute, so what the host sent is not trusted.
    expect(labelColor("red; background: url(x)")).toBeNull();
    expect(labelColor(null)).toBeNull();
    expect(labelColor("fff")).toBeNull();
  });
});

describe("summaries", () => {
  it("counts changes and comments in words that read", () => {
    expect(fileSummary(pr().changed_files)).toBe("36 files changed");
    expect(fileSummary(1)).toBe("1 file changed");
    expect(fileSummary(0)).toBe("No files changed");
    expect(commentSummary(0)).toBe("No comments");
    expect(commentSummary(1)).toBe("1 comment");
    expect(commentSummary(4)).toBe("4 comments");
  });
});
