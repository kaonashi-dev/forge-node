import { describe, expect, it } from "vitest";
import type { ActionSuggestion, ProjectAction, RepoActionsFile } from "../../contracts/runtime";
import {
  buildDraft,
  duplicateLabels,
  matchesQuery,
  neighbourIndex,
  toActionSet,
} from "./actionDraft";

function suggestion(
  label: string,
  kind: ActionSuggestion["kind"],
  extra: Partial<ActionSuggestion> = {},
): ActionSuggestion {
  return {
    label,
    command: `bun run ${label}`,
    kind,
    pinned: false,
    source: { kind: "detected" },
    detected_from: "package.json",
    saved_as: null,
    ...extra,
  };
}

function saved(
  id: string,
  label: string,
  source: ProjectAction["source"] = { kind: "manual" },
): ProjectAction {
  return {
    id,
    project_id: "p",
    label,
    command: "x",
    kind: "custom",
    single_instance: true,
    pinned: false,
    source,
    position: 0,
    created_at: "2026-01-01T00:00:00Z",
  };
}

const ids = () => {
  let n = 0;
  return () => `new-${++n}`;
};

describe("actions dialog draft", () => {
  it("prechecks nothing new and skips suggestions already saved", () => {
    const rows = buildDraft(
      "p",
      [saved("s1", "test")],
      [
        suggestion("test", "test", { saved_as: "s1" }),
        suggestion("dev", "dev", { pinned: true }),
        suggestion("lint", "lint"),
        suggestion("build", "build"),
      ],
      null,
      ids(),
    );
    expect(rows.map((r) => [r.action.label, r.origin, r.include])).toEqual([
      ["test", "saved", true],
      ["dev", "suggested", false],
      ["lint", "suggested", false],
      ["build", "suggested", false],
    ]);
  });

  it("never prechecks the repository file and offers a drifted import as an update", () => {
    const repo: RepoActionsFile = {
      file_hash: "new",
      actions: [
        suggestion("dev", "dev", { source: { kind: "repo", file_hash: "new" }, saved_as: "r1" }),
        suggestion("db", "custom", { source: { kind: "repo", file_hash: "new" } }),
      ],
      error: null,
      drift: ["r1"],
    };
    const rows = buildDraft(
      "p",
      [saved("r1", "dev", { kind: "repo", file_hash: "old" })],
      [],
      repo,
      ids(),
    );
    expect(rows.map((r) => [r.action.id, r.origin, r.include, r.drift])).toEqual([
      ["r1", "saved", true, true],
      ["r1", "repo", false, false],
      ["new-1", "repo", false, false],
    ]);

    rows[1]!.include = true;
    const set = toActionSet(rows);
    expect(set).toHaveLength(1);
    expect(set[0]!.source).toEqual({ kind: "repo", file_hash: "new" });
  });

  it("resolves suggested dependencies among what is saved and renumbers positions", () => {
    const rows = buildDraft(
      "p",
      [],
      [suggestion("dev", "dev", { depends_on: ["DB", "missing"] }), suggestion("db", "dev")],
      null,
      ids(),
    );
    for (const row of rows) row.include = true;
    const set = toActionSet(rows);
    expect(set.map((a) => [a.label, a.position])).toEqual([
      ["dev", 0],
      ["db", 1],
    ]);
    expect(set[0]!.depends_on).toEqual([set[1]!.id]);
  });

  it("reports labels that would collide ignoring case", () => {
    const rows = buildDraft("p", [saved("s", "Dev")], [suggestion("dev", "dev")], null, ids());
    expect(duplicateLabels(rows)).toEqual([]);
    rows[1]!.include = true;
    expect(duplicateLabels(rows)).toEqual(["dev"]);
    rows[1]!.include = false;
    expect(duplicateLabels(rows)).toEqual([]);
  });

  it("drops a dependency on a row that is not being saved", () => {
    const db = saved("db", "db");
    const dev = { ...saved("dev", "dev"), depends_on: ["db"] };
    const rows = buildDraft("p", [db, dev], [], null, ids());
    rows[0]!.include = false;
    expect(toActionSet(rows)[0]!.depends_on).toEqual([]);
  });

  it("filters by label, command or source file, ignoring case", () => {
    const [row] = buildDraft("p", [], [suggestion("Storybook", "dev")], null, ids());
    expect(matchesQuery(row!, "  ")).toBe(true);
    expect(matchesQuery(row!, "story")).toBe(true);
    expect(matchesQuery(row!, "package")).toBe(true);
    expect(matchesQuery(row!, "vite")).toBe(false);
  });

  it("moves among the rows a section shows, skipping hidden and other-origin rows", () => {
    const rows = buildDraft(
      "p",
      [saved("a", "api"), saved("b", "build"), saved("c", "check")],
      [suggestion("dev", "dev")],
      null,
      ids(),
    );
    const all = () => true;
    expect(neighbourIndex(rows, 2, 1, all)).toBeNull();
    expect(neighbourIndex(rows, 0, -1, all)).toBeNull();
    const notBuild = (row: (typeof rows)[number]) => row.action.label !== "build";
    expect(neighbourIndex(rows, 2, -1, notBuild)).toBe(0);
    expect(neighbourIndex(rows, 0, 1, notBuild)).toBe(2);
  });
});
