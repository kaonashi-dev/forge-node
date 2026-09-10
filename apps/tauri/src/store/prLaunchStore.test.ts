import { beforeEach, describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import { applyShellSnapshot, emptySnapshot, forgeStore } from "./forgeStore";
import { adoptPendingCompose, beginCompose, clearCompose, composeRun } from "./prComposeStore";
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
});
