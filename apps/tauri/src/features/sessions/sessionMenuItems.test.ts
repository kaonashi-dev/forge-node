import { describe, expect, it } from "vitest";
import { sessionFixture } from "../../contracts/sessions.fixture";
import type { Session } from "../../contracts/runtime";
import { sessionMenuItems } from "./sessionMenuItems";

const session = (state: Session["state"]): Session => sessionFixture({ state });

function labels(state: Session["state"]): string[] {
  return sessionMenuItems(session(state), "Terminal 1")
    .filter((item) => item.kind === "item")
    .map((item) => item.label);
}

describe("sessionMenuItems", () => {
  /* The session gestures sit between the rename and the destructive pair, so
     the run of items that can end a process stays contiguous. */
  const GESTURES = [
    "Continue in a New Session…",
    "Spawn Child…",
    "Send Context…",
    "Session Changes",
    "Review",
  ];

  it("offers kill for a running session", () => {
    expect(labels("Running")).toEqual(["Rename…", ...GESTURES, "Kill", "Close"]);
  });

  it("offers restart for a stopped session", () => {
    expect(labels({ Exited: { code: 0, signal: null } })).toEqual([
      "Rename…",
      ...GESTURES,
      "Restart",
      "Close",
    ]);
  });

  /* A review is of the session's own workspace, and only an agent's work is
     what one is for; an ended agent still left its changes behind. */
  it("offers a review for agent sessions, ended or not", () => {
    const review = (fixture: Session) =>
      sessionMenuItems(fixture, "Agent").find(
        (item) => item.kind === "item" && item.label === "Review",
      );
    const agent = sessionFixture({ kind: "Agent", agent_provider_id: "claude" });
    const ended = sessionFixture({
      kind: "Agent",
      agent_provider_id: "claude",
      state: { Exited: { code: 0, signal: null } },
    });
    expect(review(agent)).toMatchObject({ disabled: false });
    expect(review(ended)).toMatchObject({ disabled: false });
    expect(review(session("Running"))).toMatchObject({ disabled: true });
  });
});
