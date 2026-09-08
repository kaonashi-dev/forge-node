import { describe, expect, it } from "vitest";
import {
  PR_SCOPES,
  failureCopy,
  filterPullRequests,
  parseScope,
  scopeCounts,
  scopeEmptyCopy,
} from "./prFilters";
import type { PullRequest, PullRequestFailure } from "../runtime/types";

function pr(extra: Partial<PullRequest> = {}): PullRequest {
  return {
    project_id: "p1",
    repository: "acme/widget",
    host: "github.com",
    number: 1,
    title: "Add the diff view",
    body: "",
    body_truncated: false,
    url: "https://example.invalid/1",
    author: "rin",
    base_ref: "main",
    head_ref: "diff-view",
    is_draft: false,
    review_decision: null,
    labels: [],
    assignees: [],
    review_requests: [],
    additions: 0,
    deletions: 0,
    changed_files: 0,
    comment_count: 0,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    relations: { assigned: false, review_requested: false, authored: false },
    ...extra,
  };
}

const assigned = pr({
  number: 2,
  relations: { assigned: true, review_requested: false, authored: false },
});
const requested = pr({
  number: 3,
  relations: { assigned: false, review_requested: true, authored: false },
});
const authored = pr({
  number: 4,
  relations: { assigned: false, review_requested: false, authored: true },
});

describe("prFilters", () => {
  it("keeps only the relation the scope names", () => {
    const all = [pr(), assigned, requested, authored];
    const numbers = (scope: (typeof PR_SCOPES)[number]) =>
      filterPullRequests(all, scope, "", "p1").mine.map((item) => item.number);

    expect(numbers("all")).toEqual([1, 2, 3, 4]);
    expect(numbers("assigned")).toEqual([2]);
    expect(numbers("review_requested")).toEqual([3]);
    expect(numbers("authored")).toEqual([4]);
  });

  /** The scopes are not exclusive: one pull request can be in three of them. */
  it("counts a pull request under every relation it has", () => {
    const both = pr({ relations: { assigned: true, review_requested: true, authored: true } });
    expect(scopeCounts([both])).toEqual({
      all: 1,
      assigned: 1,
      review_requested: 1,
      authored: 1,
    });
  });

  it("puts this project's pull requests first and keeps the rest", () => {
    const other = pr({ number: 9, project_id: "p2", repository: "other/thing" });
    const grouped = filterPullRequests([other, pr()], "all", "", "p1");
    expect(grouped.mine.map((item) => item.number)).toEqual([1]);
    expect(grouped.others.map((item) => item.number)).toEqual([9]);
  });

  /** With no checkout on screen there is no "this project", so nothing leads. */
  it("groups everything as other when no project is focused", () => {
    const grouped = filterPullRequests([pr()], "all", "", null);
    expect(grouped.mine).toEqual([]);
    expect(grouped.others).toHaveLength(1);
  });

  it("searches the title, the repository, the author, the branches and the number", () => {
    const rows = [pr()];
    for (const needle of ["diff", "ACME", "rin", "diff-view", "#1"]) {
      expect(filterPullRequests(rows, "all", needle, "p1").mine, needle).toHaveLength(1);
    }
    expect(filterPullRequests(rows, "all", "nothing", "p1").mine).toHaveLength(0);
  });

  /** The body arrives truncated, so a hit in it would depend on where it was cut. */
  it("does not search the description", () => {
    const rows = [pr({ body: "mentions kubernetes" })];
    expect(filterPullRequests(rows, "all", "kubernetes", "p1").mine).toHaveLength(0);
  });

  it("degrades an unknown remembered scope to all", () => {
    expect(parseScope("assigned")).toBe("assigned");
    expect(parseScope("something-new")).toBe("all");
    expect(parseScope(null)).toBe("all");
  });

  /** An empty list has to say which question came back empty. */
  it("gives every scope its own empty sentence", () => {
    const sentences = PR_SCOPES.map(scopeEmptyCopy);
    expect(new Set(sentences).size).toBe(PR_SCOPES.length);
  });
});

describe("failureCopy", () => {
  function failure(extra: Partial<PullRequestFailure>): PullRequestFailure {
    return {
      host: "github.com",
      repository: null,
      kind: "HostRefused",
      message: "GitHub CLI (gh) was not found on PATH",
      ...extra,
    };
  }

  it("writes its own sentence rather than pasting the CLI's words", () => {
    const copy = failureCopy(failure({ kind: "CliMissing" }));
    expect(copy).not.toContain("PATH");
    expect(copy).not.toContain("gh");
    expect(copy).toContain("GitHub CLI");
  });

  it("names the host for the failures that are about one", () => {
    expect(failureCopy(failure({ kind: "NotSignedIn" }))).toContain("github.com");
    expect(failureCopy(failure({ kind: "TimedOut" }))).toContain("github.com");
    expect(failureCopy(failure({ kind: "HostRefused" }))).toContain("github.com");
  });

  it("names the repository when one failed under a host that answered", () => {
    expect(
      failureCopy(failure({ kind: "RepositoryRefused", repository: "acme/widget" })),
    ).toContain("acme/widget");
    // No repository shipped: still a sentence, not "undefined could not be read".
    expect(failureCopy(failure({ kind: "RepositoryRefused" }))).toContain("github.com");
  });

  it("falls back to a hostless phrase when the daemon shipped no host", () => {
    expect(failureCopy(failure({ kind: "TimedOut", host: null }))).toBe(
      "The host did not answer in time.",
    );
    expect(failureCopy(failure({ kind: "HostRefused", host: "  " }))).toBe(
      "The host could not be read.",
    );
  });

  it("degrades to the daemon's text for a kind this build does not know", () => {
    const unknown = failure({ kind: "Unknown", message: "something new went wrong" });
    expect(failureCopy(unknown)).toBe("something new went wrong");
  });
});
