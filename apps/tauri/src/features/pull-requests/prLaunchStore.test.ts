import { beforeEach, describe, expect, it } from "vitest";
import { sessionFixture } from "../../contracts/sessions.fixture";
import { applyShellSnapshot, emptySnapshot, forgeStore } from "../../state/forgeStore";
import {
  adoptPendingCompose,
  beginCompose,
  beginOpening,
  clearCompose,
  clearDraft,
  composeBusy,
  composeRun,
  finishOpening,
  prComposeStore,
  receiveDraft,
} from "./prComposeStore";
import { adoptPendingReviews, beginReview, clearReview, reviewRun } from "./prReviewStore";

beforeEach(() => {
  clearReview("pr:1");
  clearCompose();
  applyShellSnapshot(emptySnapshot());
});

describe("prReviewStore adoption", () => {
  it("binds a pending review when the session lands after the tab is gone", () => {
    beginReview("pr:1", "w1");
    expect(reviewRun("pr:1")?.session).toBeNull();

    // Older agent in the same checkout must not be mistaken for this launch.
    applyShellSnapshot({
      ...emptySnapshot(),
      sessions: [
        sessionFixture({
          id: "old",
          workspace_id: "w1",
          agent_provider_id: "claude",
          created_at: "2020-01-01T00:00:00.000Z",
        }),
        sessionFixture({
          id: "fresh",
          workspace_id: "w1",
          agent_provider_id: "claude",
          created_at: new Date(Date.now() + 1000).toISOString(),
        }),
      ],
    });
    adoptPendingReviews(forgeStore.sessions);

    expect(reviewRun("pr:1")?.session).toBe("fresh");
  });

  it("ignores sessions in another checkout", () => {
    beginReview("pr:1", "w1");
    applyShellSnapshot({
      ...emptySnapshot(),
      sessions: [
        sessionFixture({
          id: "elsewhere",
          workspace_id: "w2",
          agent_provider_id: "claude",
          created_at: new Date(Date.now() + 1000).toISOString(),
        }),
      ],
    });
    adoptPendingReviews(forgeStore.sessions);
    expect(reviewRun("pr:1")?.session).toBeNull();
  });
});

describe("prComposeStore adoption", () => {
  it("binds a pending compose launch without the tab being mounted", () => {
    beginCompose("w1");
    expect(composeRun()?.session).toBeNull();

    applyShellSnapshot({
      ...emptySnapshot(),
      sessions: [
        sessionFixture({
          id: "agent",
          workspace_id: "w1",
          agent_provider_id: "claude",
          created_at: new Date(Date.now() + 1000).toISOString(),
        }),
      ],
    });
    adoptPendingCompose(forgeStore.sessions);

    expect(composeRun()?.session).toBe("agent");
  });

  it("frees the checkout once the adopted agent exits or is closed", () => {
    const run = { startedAt: "2026-01-01T00:00:00Z", session: "agent", workspace: "w1" };
    const agent = (state: "Running" | { Exited: { code: number; signal: null } }) => [
      sessionFixture({ id: "agent", workspace_id: "w1", agent_provider_id: "claude", state }),
    ];
    expect(composeBusy(run, agent("Running"))).toBe(true);
    expect(composeBusy(run, agent({ Exited: { code: 0, signal: null } }))).toBe(false);
    expect(composeBusy(run, [])).toBe(false);
    expect(composeBusy({ ...run, session: null }, [])).toBe(true);
  });
});

describe("prComposeStore drafts", () => {
  const received = {
    workspace: "w1",
    session: "agent",
    draft: { title: "T", body: "B", branch: "fix-x", commit_message: "Fix x" },
  };

  it("keeps the draft when opening fails so the user can retry", () => {
    receiveDraft(received);
    beginOpening("w1");
    finishOpening("w1", false);
    expect(prComposeStore.opening).toBeNull();
    expect(prComposeStore.draft).not.toBeNull();
    clearDraft();
  });

  it("consumes the draft once its pull request opens", () => {
    receiveDraft(received);
    beginOpening("w1");
    finishOpening("w2", true);
    expect(prComposeStore.draft).not.toBeNull();
    finishOpening("w1", true);
    expect(prComposeStore.opening).toBeNull();
    expect(prComposeStore.draft).toBeNull();
  });
});
