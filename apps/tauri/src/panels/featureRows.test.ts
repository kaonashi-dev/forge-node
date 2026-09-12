import { describe, expect, it } from "vitest";
import {
  agentCount,
  checkoutLabel,
  clip,
  counts,
  detailRows,
  emptyReason,
  lastActivity,
  matchesSearch,
  metaFacts,
  plural,
  relativeAge,
  visibleFeatures,
} from "./featureRows";
import type { HarnessFeature } from "../harness/types";
import type { Job, Session, Workspace } from "../runtime/types";

const NOW = new Date("2026-08-30T12:00:00Z");

function feature(id: number, status: string, extra: Partial<HarnessFeature> = {}): HarnessFeature {
  return {
    id,
    slug: `f${id}`,
    title: null,
    spec_raw: null,
    status,
    review_rounds: null,
    gate_attempts: null,
    crates: null,
    acceptance: null,
    source_issue: null,
    created_at: null,
    orchestrator_session_id: null,
    workspace_id: null,
    workspace_path: null,
    revision: null,
    attempts: null,
    blocked_reason: null,
    ...extra,
  };
}

function session(id: string, parent: string | null): Session {
  return {
    id,
    workspace_id: "w1",
    kind: "Agent",
    role: parent === null ? "Orchestrator" : "Executor",
    parent_session_id: parent,
    root_session_id: parent ?? id,
    title: { user: null, terminal: id },
    state: "Running",
    terminal_id: null,
    agent_provider_id: "claude",
    agent_profile_id: null,
    created_at: "2026-08-30T10:00:00Z",
  };
}

function job(id: string, featureId: number, at: string, finished: string | null = null): Job {
  return {
    id,
    provider_id: "claude",
    workspace_id: "w1",
    role: "Executor",
    feature_id: featureId,
    parent_session_id: null,
    state: "Running",
    summary: "",
    prompt: "",
    provider_session_id: null,
    exit_code: null,
    last_line: null,
    started_at: at,
    finished_at: finished,
    log_path: "/tmp/j.log",
  };
}

const workspaces: Workspace[] = [
  {
    id: "w1",
    project_id: "p",
    kind: "Worktree",
    path: "/tmp/forge/w1",
    branch: "feature/grok",
    display_name: null,
    managed_by_app: true,
    status: { dirty: false, head: null, ahead: null, behind: null, measured_at: null },
  },
];

describe("features panel rows", () => {
  // The work that is still yours comes before the work that is finished; ids
  // order each half because `created_at` is a bare date.
  it("sorts open features above finished ones, newest first", () => {
    const rows = visibleFeatures({
      features: [
        feature(1, "done"),
        feature(2, "in_progress"),
        feature(3, "blocked"),
        feature(4, "spec_ready"),
      ],
      scope: "Project",
      anchor: null,
      filter: "All",
      search: "",
    });
    expect(rows.map((row) => row.id)).toEqual([4, 2, 3, 1]);
  });

  // A scope measured from nowhere would otherwise be an empty panel.
  it("lets the narrow scope through when nothing is selected", () => {
    const features = [feature(1, "done", { workspace_id: "w1" }), feature(2, "done")];
    const narrow = { features, scope: "Workspace" as const, filter: "All" as const, search: "" };
    expect(visibleFeatures({ ...narrow, anchor: null }).map((f) => f.id)).toEqual([2, 1]);
    expect(visibleFeatures({ ...narrow, anchor: "w1" }).map((f) => f.id)).toEqual([1]);
  });

  it("searches the id, the slug, the title and the spec", () => {
    const row = feature(12, "done", { title: "Add Grok", spec_raw: "wire the provider" });
    expect(matchesSearch(row, "12")).toBe(true);
    expect(matchesSearch(row, "f12")).toBe(true);
    expect(matchesSearch(row, "grok")).toBe(true);
    expect(matchesSearch(row, "provider")).toBe(true);
    expect(matchesSearch(row, "nothing")).toBe(false);
  });

  // With the orchestrator gone from the store there is no tree to count.
  it("counts the orchestrator's tree plus the steps, and nothing when the root is gone", () => {
    const row = feature(4, "in_progress", { orchestrator_session_id: "s1" });
    const sessions = [session("s1", null), session("s2", "s1")];
    const jobs = [job("j1", 4, "2026-08-30T11:00:00Z")];
    expect(agentCount(sessions, jobs, row)).toBe(3);
    expect(agentCount([session("s2", "s1")], jobs, row)).toBe(1);
    expect(agentCount(sessions, jobs, feature(5, "done"))).toBe(0);
  });

  it("takes recency from the steps, or reports none", () => {
    const jobs = [
      job("j1", 4, "2026-08-30T10:00:00Z", "2026-08-30T11:00:00Z"),
      job("j2", 4, "2026-08-30T10:30:00Z"),
    ];
    expect(lastActivity(jobs, 4)).toBe("2026-08-30T11:00:00Z");
    expect(lastActivity(jobs, 9)).toBeNull();
  });

  it("names a checkout only when the feature claims one", () => {
    expect(checkoutLabel(workspaces, feature(1, "done", { workspace_id: "w1" }))).toBe(
      "feature/grok",
    );
    expect(checkoutLabel(workspaces, feature(1, "done"))).toBeNull();
  });

  // A card whose title fell back to the slug would otherwise print it twice.
  it("carries only the facts a feature has, and never the slug twice", () => {
    const bare = metaFacts(feature(1, "pending"), [], [], workspaces, NOW);
    expect(bare).toEqual([]);
    const full = metaFacts(
      feature(2, "in_progress", { title: "Add Grok", workspace_id: "w1" }),
      [],
      [job("j1", 2, "2026-08-30T10:00:00Z", "2026-08-30T11:00:00Z")],
      workspaces,
      NOW,
    );
    expect(full).toEqual(["f2", "1 agent", "feature/grok", "1h ago"]);
  });

  it("lists a feature's counters only once they are non-zero", () => {
    expect(detailRows(feature(1, "pending"), workspaces)).toEqual([
      { label: "Checkout", value: "not claimed" },
    ]);
    expect(
      detailRows(
        feature(2, "blocked", {
          created_at: "2026-08-28",
          review_rounds: 2,
          gate_attempts: 0,
          source_issue: 41,
          blocked_reason: "test data",
        }),
        workspaces,
      ).map((row) => row.label),
    ).toEqual(["Checkout", "Registered", "Review rounds", "From issue", "Blocked"]);
  });

  it("reports the two numbers and whether it is still reading", () => {
    expect(counts(3, 9, false)).toBe("3 shown · 9 tracked");
    expect(counts(3, 9, true)).toBe("3 shown · 9 tracked · loading…");
  });

  it("says what the list is empty of", () => {
    expect(emptyReason(null, true, 0, "").title).toBe("No project selected");
    expect(emptyReason("p", false, 0, "").title).toBe("Harness not initialized");
    expect(emptyReason("p", true, 0, "").title).toBe("No features yet");
    expect(emptyReason("p", true, 4, "grok").title).toBe("Nothing matches “grok”");
    expect(emptyReason("p", true, 4, "").title).toBe("No features in this scope");
  });

  it("reads plurals as English and marks only what it cut", () => {
    expect(plural(1, "agent")).toBe("1 agent");
    expect(plural(2, "agent")).toBe("2 agents");
    expect(clip("  short  ", 10)).toBe("short");
    expect(clip("abcdefghij", 4)).toBe("abcd…");
  });

  it("gives the coarsest age that is still true", () => {
    expect(relativeAge("2026-08-30T11:55:00Z", NOW)).toBe("5m");
    expect(relativeAge("2026-08-30T09:00:00Z", NOW)).toBe("3h");
    expect(relativeAge("2026-08-27T12:00:00Z", NOW)).toBe("3d");
  });
});
