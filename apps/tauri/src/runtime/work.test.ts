import { describe, expect, it } from "vitest";
import { sessionFixture } from "./sessions.fixture";
import { sessionWork, WORKING_WINDOW_MS } from "./work";

const NOW = Date.parse("2026-08-30T12:00:00Z");
const quiet = { wants_you: false };

function agent(lastActivity: string | undefined, partial = {}) {
  return sessionFixture({
    agent_provider_id: "claude",
    kind: "Agent",
    last_activity_at: lastActivity,
    created_at: "2026-08-30T11:00:00Z",
    ...partial,
  });
}

describe("sessionWork", () => {
  it("is working while the agent is still producing output", () => {
    const recent = new Date(NOW - WORKING_WINDOW_MS + 5_000).toISOString();
    expect(sessionWork(agent(recent), quiet, NOW)).toBe("working");
  });

  it("is idle once it has been quiet for the whole window", () => {
    const stale = new Date(NOW - WORKING_WINDOW_MS - 1_000).toISOString();
    expect(sessionWork(agent(stale), quiet, NOW)).toBe("idle");
  });

  it("puts needing a person ahead of either", () => {
    const stale = new Date(NOW - WORKING_WINDOW_MS - 1_000).toISOString();
    expect(sessionWork(agent(stale), { wants_you: true }, NOW)).toBe("needs-you");
  });

  it("never calls a shell working or idle: nothing was asked of it", () => {
    const shell = sessionFixture({
      agent_provider_id: null,
      last_activity_at: "2026-08-30T09:00:00Z",
    });
    expect(sessionWork(shell, quiet, NOW)).toBe("running");
  });

  it("reports the process before the work when the process is the news", () => {
    expect(sessionWork(agent(undefined, { state: "Starting" }), quiet, NOW)).toBe("starting");
    expect(sessionWork(agent(undefined, { state: "Orphaned" }), quiet, NOW)).toBe("failed");
    expect(sessionWork(agent(undefined, { state: { Failed: { code: 1 } } }), quiet, NOW)).toBe(
      "failed",
    );
    expect(sessionWork(agent(undefined, { state: "Exited" }), quiet, NOW)).toBe("exited");
  });

  it("falls back to the creation stamp, and to working when neither parses", () => {
    expect(sessionWork(agent(undefined), quiet, NOW)).toBe("idle");
    expect(sessionWork(agent("not a date"), quiet, NOW)).toBe("working");
  });

  it("outranks a stale clock with an attention flag even while starting", () => {
    expect(sessionWork(agent(undefined, { state: "Starting" }), { wants_you: true }, NOW)).toBe(
      "starting",
    );
  });
});
