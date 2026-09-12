import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { PullRequest, Session, ShellSnapshot, Workspace } from "../runtime/types";
import {
  buildTree,
  railCollapseTarget,
  railExpandTarget,
  railRows,
  waiting,
  type GroupNode,
  type ProjectNode,
  type SessionNode,
  type WorkspaceNode,
} from "./tree";

function session(id: string, workspace: string, extra: Partial<Session> = {}): Session {
  return sessionFixture({
    id,
    workspace_id: workspace,
    kind: "Agent",
    title: { user: null, terminal: id },
    terminal_id: `t-${id}`,
    agent_provider_id: "claude",
    created_at: `2026-01-0${id.length}T00:00:00Z`,
    ...extra,
  });
}

function checkout(id: string, project: string, extra: Partial<Workspace> = {}): Workspace {
  return {
    id,
    project_id: project,
    kind: "Main",
    path: `/r/${id}`,
    branch: null,
    display_name: null,
    managed_by_app: true,
    status: { dirty: false, head: null, ahead: null, behind: null, measured_at: null },
    ...extra,
  };
}

function prFixture(extra: Partial<PullRequest>): PullRequest {
  return {
    project_id: null,
    repository: "acme/forge",
    host: "github.com",
    number: 1,
    title: "Title",
    body: "",
    body_truncated: false,
    url: "https://example.invalid/1",
    author: "someone",
    base_ref: "main",
    head_ref: "main",
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

function snapshot(): ShellSnapshot {
  return {
    ...emptySnapshot(),
    project_groups: [{ id: "g1", name: "Work" }],
    projects: [
      { id: "p1", project_group_id: "g1", name: "forge", icon: null, root_path: "/r" },
      { id: "p2", project_group_id: null, name: "loose", icon: null, root_path: "/l" },
    ],
    workspaces: [
      {
        id: "w1",
        project_id: "p1",
        kind: "worktree",
        path: "/r/main",
        branch: "main",
        display_name: null,
        managed_by_app: true,
        status: {
          dirty: true,
          head: null,
          ahead: null,
          behind: null,
          measured_at: "2026-01-01T00:00:00Z",
        },
      },
      {
        id: "w2",
        project_id: "p2",
        kind: "repo",
        path: "/l/",
        branch: null,
        display_name: null,
        managed_by_app: false,
        status: { dirty: true, head: null, ahead: null, behind: null, measured_at: null },
      },
    ],
    sessions: [session("a", "w1"), session("b", "w2")],
    session_attention: { a: { wants_you: true, unread: false } },
  };
}

describe("buildTree", () => {
  it("nests group → project → workspace → session", () => {
    const tree = buildTree(snapshot());
    expect(tree[0].name).toBe("Work");
    expect(tree[0].projects[0].name).toBe("forge");
    expect(tree[0].projects[0].workspaces[0].label).toBe("main");
    expect(tree[0].projects[0].workspaces[0].sessions[0].session.id).toBe("a");
  });

  // A named group is something the user made; it should not be pushed below
  // the projects that belong to none.
  it("puts the ungrouped projects last, in an unnamed group", () => {
    const tree = buildTree(snapshot());
    expect(tree.at(-1)?.id).toBeNull();
    expect(tree.at(-1)?.projects[0].name).toBe("loose");
  });

  it("keeps a named group visible while it is empty", () => {
    const store = { ...snapshot(), projects: [] };
    expect(buildTree(store)).toEqual([{ id: "g1", name: "Work", projects: [], wantsYou: false }]);
  });

  // Collapsing a checkout must never be able to hide a question (§16.3).
  it("rolls a waiting session all the way up to the group", () => {
    const tree = buildTree(snapshot());
    expect(tree[0].wantsYou).toBe(true);
    expect(tree[0].projects[0].wantsYou).toBe(true);
    expect(tree[0].projects[0].workspaces[0].wantsYou).toBe(true);
    expect(tree.at(-1)?.wantsYou).toBe(false);
  });

  // `measured_at: None` means "not measured", which is not the same as clean —
  // and it must not be rendered as either.
  it("keeps unmeasured apart from clean", () => {
    const tree = buildTree(snapshot());
    expect(tree[0].projects[0].workspaces[0].measured).toBe(true);
    expect(tree.at(-1)?.projects[0].workspaces[0].measured).toBe(false);
  });

  it("names a workspace with no branch after its directory", () => {
    expect(buildTree(snapshot()).at(-1)?.projects[0].workspaces[0].label).toBe("l");
  });

  // A first paint with no stored order must not follow HashMap walk order.
  it("leads with the repository's own checkout, then worktrees by name", () => {
    const store = snapshot();
    store.workspaces = [
      checkout("wz", "p1", { kind: "GitWorktree", branch: "zeta" }),
      checkout("wa", "p1", { kind: "GitWorktree", branch: "alpha" }),
      checkout("wm", "p1", { kind: "Main", branch: "main" }),
    ];
    const rows = buildTree(store)[0].projects[0].workspaces;
    expect(rows.map((row) => row.label)).toEqual(["main", "alpha", "zeta"]);
    expect(rows.map((row) => row.worktree)).toEqual([false, true, true]);
  });

  it("honours a stored checkout order, including a worktree above primary", () => {
    const store = snapshot();
    store.workspaces = [
      checkout("wm", "p1", { kind: "Main", branch: "main" }),
      checkout("wt", "p1", { kind: "GitWorktree", branch: "test" }),
    ];
    const rows = buildTree(store, { p1: ["wt", "wm"] })[0].projects[0].workspaces;
    expect(rows.map((row) => row.label)).toEqual(["test", "main"]);
  });

  // Two repositories in one window can both have a `main`, and a PR badge on
  // the wrong checkout is worse than none.
  it("matches a pull request on head ref and project together", () => {
    const store = snapshot();
    store.pull_requests = {
      ...store.pull_requests,
      pull_requests: [
        prFixture({ project_id: "p2", head_ref: "main", number: 9 }),
        prFixture({ project_id: "p1", head_ref: "main", number: 7, is_draft: true }),
      ],
    };
    const found = buildTree(store)[0].projects[0].workspaces[0].pullRequest;
    expect(found).toEqual({ number: 7, draft: true, decision: null, title: "Title" });
  });

  it("leaves a branch with no pull request unmarked", () => {
    expect(buildTree(snapshot())[0].projects[0].workspaces[0].pullRequest).toBeNull();
  });
});

describe("waiting", () => {
  // Longest-waiting leads: that is the one at risk of being forgotten, and a
  // rail sorted by recency would keep burying it.
  it("puts the one that has waited longest first", () => {
    const store: ShellSnapshot = {
      ...snapshot(),
      sessions: [
        session("new", "w1", { created_at: "2026-02-01T00:00:00Z" }),
        session("old", "w1", { created_at: "2026-01-01T00:00:00Z" }),
      ],
      session_attention: {
        new: { wants_you: true, unread: false },
        old: { wants_you: true, unread: false },
      },
    };
    expect(waiting(store).map((s) => s.id)).toEqual(["old", "new"]);
  });

  it("lists only what actually asked", () => {
    expect(waiting(snapshot()).map((s) => s.id)).toEqual(["a"]);
  });
});

describe("buildTree session nesting", () => {
  // The harness starts an orchestrator and hangs its steps off it. Flat, those
  // rows read as unrelated agents rather than one piece of work.
  it("indents a session under its parent", () => {
    const snap = snapshot();
    snap.sessions = [
      session("orch", "w1", { role: "Orchestrator", created_at: "2026-01-01T00:00:00Z" }),
      session("kid", "w1", {
        parent_session_id: "orch",
        root_session_id: "orch",
        role: "Executor",
        created_at: "2026-01-01T00:00:01Z",
      }),
    ];
    const rows = buildTree(snap)[0].projects[0].workspaces[0].sessions;
    expect(rows.map((row) => [row.session.id, row.depth])).toEqual([
      ["orch", 0],
      ["kid", 1],
    ]);
  });

  it("labels a role-carrying session with its role", () => {
    const snap = snapshot();
    snap.sessions = [
      session("orch", "w1", { role: "Orchestrator", title: { user: "spec", terminal: null } }),
    ];
    const rows = buildTree(snap)[0].projects[0].workspaces[0].sessions;
    expect(rows[0].label).toBe("Orchestrator · spec");
  });
});

describe("railRows", () => {
  const session = (id: string, depth = 0): SessionNode => ({
    id,
    session: { id } as never,
    label: id,
    depth,
    wantsYou: false,
    unread: false,
  });
  const workspace = (id: string, sessions: SessionNode[]): WorkspaceNode =>
    ({ id, label: id, path: `/${id}`, sessions, wantsYou: false }) as WorkspaceNode;
  const project = (id: string, workspaces: WorkspaceNode[]): ProjectNode =>
    ({ id, name: id, icon: null, workspaces, wantsYou: false }) as ProjectNode;
  const grouped = (name: string | null, projects: ProjectNode[]): GroupNode =>
    ({ id: name, name, projects, wantsYou: false }) as GroupNode;

  const tree = [grouped(null, [project("p", [workspace("w", [session("s")])])])];

  it("flattens the four levels into paint order", () => {
    expect(railRows(tree, new Set()).map((row) => row.id)).toEqual([
      "project:p",
      "workspace:w",
      "session:s",
    ]);
  });

  it("gives the ungrouped group no row of its own", () => {
    // There is nothing to fold and nothing to name; a row for it would be a
    // stop on every `↓` that says nothing.
    expect(railRows(tree, new Set()).some((row) => row.kind === "group")).toBe(false);
  });

  it("hides the children of a folded row entirely", () => {
    // Not marked hidden — absent. A row that cannot be seen must not be
    // reachable by an arrow key.
    expect(railRows(tree, new Set(["project:p"])).map((row) => row.id)).toEqual(["project:p"]);
  });

  it("counts depth from the group, so an ungrouped project starts at zero", () => {
    expect(railRows(tree, new Set())[0].depth).toBe(0);
    const inGroup = [grouped("Work", [project("p", [])])];
    expect(railRows(inGroup, new Set()).map((row) => row.depth)).toEqual([0, 1]);
  });

  it("carries a nested agent's own depth through", () => {
    const nested = [grouped(null, [project("p", [workspace("w", [session("s", 2)])])])];
    expect(railRows(nested, new Set()).at(-1)?.depth).toBe(4);
  });
});

describe("railCollapseTarget", () => {
  const rows = railRowsFixture();

  it("folds an open row", () => {
    expect(railCollapseTarget(rows[0])).toEqual({ fold: "project:p" });
  });

  it("moves to the parent of a row that is already folded", () => {
    const folded = { ...rows[0], expanded: false };
    expect(railCollapseTarget(folded)).toBeNull();
    const child = { ...rows[1], expanded: false };
    expect(railCollapseTarget(child)).toEqual({ select: "project:p" });
  });

  it("moves a session — which cannot fold — to its workspace", () => {
    expect(railCollapseTarget(rows[2])).toEqual({ select: "workspace:w" });
  });

  it("has nowhere to go from a top-level folded row", () => {
    expect(railCollapseTarget(undefined)).toBeNull();
  });
});

describe("railExpandTarget", () => {
  const rows = railRowsFixture();

  it("unfolds a folded row", () => {
    const folded = [{ ...rows[0], expanded: false }];
    expect(railExpandTarget(folded[0], folded, 0)).toEqual({ unfold: "project:p" });
  });

  it("moves into the first child of an open row", () => {
    expect(railExpandTarget(rows[0], rows, 0)).toEqual({ select: "workspace:w" });
  });

  it("does nothing on a session, which has no children", () => {
    expect(railExpandTarget(rows[2], rows, 2)).toBeNull();
  });
});

function railRowsFixture() {
  const session = (id: string): SessionNode => ({
    id,
    session: { id } as never,
    label: id,
    depth: 0,
    wantsYou: false,
    unread: false,
  });
  return railRows(
    [
      {
        id: null,
        name: null,
        wantsYou: false,
        projects: [
          {
            id: "p",
            name: "p",
            icon: null,
            wantsYou: false,
            workspaces: [
              { id: "w", label: "w", path: "/w", sessions: [session("s")] } as WorkspaceNode,
            ],
          },
        ],
      } as GroupNode,
    ],
    new Set(),
  );
}
