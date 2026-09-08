import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { Session, ShellSnapshot, Workspace } from "../runtime/types";
import {
  BRANCHES,
  FILES,
  SESSIONS,
  admits,
  fileEntries,
  paletteEntries,
  rank,
  type PaletteEntry,
} from "./entries";

function workspace(id: string, branch: string | null): Workspace {
  return {
    id,
    project_id: "p1",
    kind: "worktree",
    path: `/r/${id}`,
    branch,
    display_name: null,
    managed_by_app: true,
    status: { dirty: false, ahead: null, behind: null, measured_at: null },
  };
}

function session(id: string, ws: string, extra: Partial<Session> = {}): Session {
  return sessionFixture({
    id,
    workspace_id: ws,
    title: { user: null, terminal: "~/d/forge-node" },
    terminal_id: `t-${id}`,
    state: "Running",
    ...extra,
  });
}

function snapshot(sessions: Session[]): ShellSnapshot {
  return {
    ...emptySnapshot(),
    projects: [
      { id: "p1", project_group_id: null, name: "forge-node", icon: null, root_path: "/r" },
    ],
    workspaces: [workspace("w1", "main"), workspace("w2", "test5")],
    sessions,
  };
}

const groupsOf = (entries: PaletteEntry[]) => [...new Set(entries.map((entry) => entry.group))];

describe("palette scopes", () => {
  it("lets ⌘⇧O reach files as well as sessions and checkouts", () => {
    expect(admits("places", FILES)).toBe(true);
    expect(admits("places", SESSIONS)).toBe(true);
    // The command half stays out of it: "main" is a branch and a substring of
    // two commands, which is the whole reason the scope exists.
    expect(admits("places", "Commands")).toBe(false);
  });
});

describe("session rows", () => {
  it("sinks the sessions that are no longer running", () => {
    const entries = paletteEntries(
      snapshot([
        session("a", "w1", { state: { Exited: { code: 0, signal: null } } }),
        session("b", "w1"),
      ]),
      null,
    ).filter((entry) => entry.group === SESSIONS);
    expect(entries.map((entry) => entry.enabled)).toEqual([true, false]);
  });

  it("says what became of a session that is not running", () => {
    const [entry] = paletteEntries(
      snapshot([session("a", "w1", { state: { Exited: { code: 1, signal: null } } })]),
      null,
    ).filter((item) => item.group === SESSIONS);
    expect(entry.note).toBe("main · exited 1");
  });

  it("finds one of several identical titles by its branch", () => {
    const entries = paletteEntries(snapshot([session("a", "w1"), session("b", "w2")]), null);
    const found = rank(entries, "test5").filter((entry) => entry.group === SESSIONS);
    expect(found).toHaveLength(1);
    expect(found[0].choice).toEqual({ kind: "focus_session", session: "b" });
  });

  it("leaves the session already on screen out of the list", () => {
    const entries = paletteEntries(snapshot([session("a", "w1"), session("b", "w2")]), "a");
    const sessions = entries.filter((entry) => entry.group === SESSIONS);
    expect(sessions).toHaveLength(1);
  });
});

describe("rank", () => {
  const places = () => paletteEntries(snapshot([session("a", "w1")]), null);
  const files = () => fileEntries("w1", ["src/palette/entries.ts", "README.md"]);

  it("keeps each group contiguous, in presentation order", () => {
    const ranked = rank([...files(), ...places()], "");
    // No launchables in the fixture, so no "Create" — the rest is the order
    // the palette lists them in, whatever order they were built in.
    expect(groupsOf(ranked)).toEqual([SESSIONS, BRANCHES, FILES, "Commands"]);
  });

  it("still groups once a query has reordered the rows", () => {
    const ranked = rank([...places(), ...files()], "e");
    const groups = ranked.map((entry) => entry.group);
    // Contiguous means every group's rows sit in one run: the palette prints a
    // heading where the group changes, so a second run would be filed wrong.
    expect(groups).toEqual([...new Set(groups)].flatMap((g) => groups.filter((x) => x === g)));
  });

  it("matches a file on its path, not only its basename", () => {
    const ranked = rank(files(), "srcpal");
    expect(ranked.map((entry) => entry.note)).toEqual(["src/palette/entries.ts"]);
  });

  it("caps the files it lists", () => {
    const many = fileEntries(
      "w1",
      Array.from({ length: 500 }, (_, index) => `src/file-${index}.ts`),
    );
    expect(rank(many, "").length).toBe(200);
  });
});
