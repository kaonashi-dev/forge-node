import { describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { SessionNode, WorkspacePullRequest } from "./tree";
import { chipGroups, prLabel, prTone, rollupWork, syncLabel } from "./workspaceCard";

function node(id: string, provider: string | null): SessionNode {
  return {
    session: sessionFixture({ id, agent_provider_id: provider }),
    label: id,
    depth: 0,
    wantsYou: false,
    unread: false,
  };
}

function pr(extra: Partial<WorkspacePullRequest> = {}): WorkspacePullRequest {
  return { number: 12, draft: false, decision: null, title: "Delete strategy", ...extra };
}

describe("rollupWork", () => {
  // A folded card must never be able to hide a question (§16.3).
  it("lets a waiting session outrank everything healthy", () => {
    expect(rollupWork(["working", "running", "needs-you", "idle"])).toBe("needs-you");
  });

  it("prefers a crash to anything still alive", () => {
    expect(rollupWork(["idle", "failed", "running"])).toBe("failed");
  });

  // A finished session says nothing about a checkout that also has a live one.
  it("puts exited last", () => {
    expect(rollupWork(["exited", "running"])).toBe("running");
  });

  it("has no answer for a checkout with nothing in it", () => {
    expect(rollupWork([])).toBeNull();
  });
});

describe("syncLabel", () => {
  it("says nothing when the branch has not diverged", () => {
    expect(syncLabel(0, 0)).toBeNull();
    expect(syncLabel(null, null)).toBeNull();
  });

  it("shows each direction it has", () => {
    expect(syncLabel(2, null)).toBe("↑2");
    expect(syncLabel(2, 1)).toBe("↑2 ↓1");
  });
});

describe("prTone", () => {
  // A draft is open, but nobody is being asked anything yet.
  it("keeps a draft neutral whatever the review says", () => {
    expect(prTone(pr({ draft: true, decision: "Approved" }))).toBe("neutral");
  });

  it("colours the decision", () => {
    expect(prTone(pr({ decision: "Approved" }))).toBe("good");
    expect(prTone(pr({ decision: "ChangesRequested" }))).toBe("bad");
    expect(prTone(pr({ decision: "ReviewRequired" }))).toBe("warn");
    expect(prTone(pr())).toBe("neutral");
  });

  it("titles the mark with what a hover should say", () => {
    expect(prLabel(pr({ decision: "Approved" }))).toBe("#12 · approved · Delete strategy");
    expect(prLabel(pr())).toBe("#12 · Delete strategy");
  });
});

describe("chipGroups", () => {
  it("splits agents from terminals", () => {
    const groups = chipGroups([node("a", "claude"), node("t", null), node("b", "codex")]);
    expect(groups.agents.map((item) => item.label)).toEqual(["a", "b"]);
    expect(groups.shells.map((item) => item.label)).toEqual(["t"]);
  });
});
