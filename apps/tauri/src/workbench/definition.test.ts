import { describe, expect, it } from "vitest";
import { MAX_CANDIDATES, candidateLabel, resolveDefinition } from "./definition";
import type { SearchMatch, SearchResults } from "./types";

function hit(path: string, line: number): SearchMatch {
  return { path, line, column: 1, text: `fn thing() {}` };
}

function results(query: string, matches: SearchMatch[], truncated = false): SearchResults {
  return { workspace_id: "w1", query, matches, truncated };
}

const here = { path: "src/a.ts", line: 10 };

describe("resolveDefinition", () => {
  it("jumps when there is one answer", () => {
    const answer = resolveDefinition(results("thing", [hit("src/b.ts", 3)]), "thing", here);
    expect(answer).toEqual({ kind: "jump", target: hit("src/b.ts", 3) });
  });

  it("offers the choice rather than picking one", () => {
    const answer = resolveDefinition(
      results("thing", [hit("src/b.ts", 3), hit("src/c.ts", 4)]),
      "thing",
      here,
    );
    expect(answer.kind).toBe("choose");
  });

  /*
   * The answer carries the query and no request id, so a lookup started before
   * the previous one landed can only be told apart by this. Without the guard
   * the second `cmd-b` jumps to the first one's answer.
   */
  it("drops an answer to a different question", () => {
    expect(resolveDefinition(results("other", [hit("src/b.ts", 3)]), "thing", here).kind).toBe(
      "none",
    );
    expect(resolveDefinition(null, "thing", here).kind).toBe("none");
  });

  it("does not offer the line the caret is already on", () => {
    expect(resolveDefinition(results("thing", [hit("src/a.ts", 10)]), "thing", here).kind).toBe(
      "none",
    );
    const answer = resolveDefinition(
      results("thing", [hit("src/a.ts", 10), hit("src/b.ts", 3)]),
      "thing",
      here,
    );
    expect(answer).toEqual({ kind: "jump", target: hit("src/b.ts", 3) });
  });

  it("caps the list and says it was capped", () => {
    const many = Array.from({ length: MAX_CANDIDATES + 5 }, (_, i) => hit("src/b.ts", i + 1));
    const answer = resolveDefinition(results("thing", many), "thing", here);
    expect(answer).toMatchObject({ kind: "choose", truncated: true });
    if (answer.kind === "choose") expect(answer.targets).toHaveLength(MAX_CANDIDATES);
  });

  // The daemon's own cap has to reach the person too: two answers out of two
  // hundred read differently from two out of two.
  it("carries the service's truncation into the choice", () => {
    const answer = resolveDefinition(
      results("thing", [hit("src/b.ts", 3), hit("src/c.ts", 4)], true),
      "thing",
      here,
    );
    expect(answer).toMatchObject({ kind: "choose", truncated: true });
  });
});

describe("candidateLabel", () => {
  it("is path and line", () => {
    expect(candidateLabel(hit("src/b.ts", 12))).toBe("src/b.ts:12");
  });
});
