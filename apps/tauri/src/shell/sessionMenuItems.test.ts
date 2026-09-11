import { describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { Session } from "../runtime/types";
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
    "Review This Checkout",
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
});
