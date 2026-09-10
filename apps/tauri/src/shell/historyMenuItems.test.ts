import { describe, expect, it } from "vitest";
import type { ExternalAgentSession } from "../runtime/types";
import type { MenuItem } from "../ui/types";
import { handoffBlocked, historyMenuItems } from "./historyMenuItems";

function session(overrides: Partial<ExternalAgentSession> = {}): ExternalAgentSession {
  return {
    session_id: "ses-1",
    project_id: "p-1",
    workspace_id: "w-1",
    provider: "claude",
    profile_id: null,
    title: "auth refactor",
    branch: "main",
    preview: null,
    model: null,
    message_count: 3,
    subagent_count: 0,
    transcript_path: "/tmp/ses-1.jsonl",
    store: "File",
    started_at: "2026-01-01T00:00:00Z",
    last_activity: "2026-01-02T00:00:00Z",
    ...overrides,
  };
}

type Action = Extract<MenuItem, { kind: "item" }>;

function actions(overrides: Partial<ExternalAgentSession> = {}): Action[] {
  return historyMenuItems(session(overrides), () => {}).filter(
    (item): item is Action => item.kind === "item",
  );
}

describe("historyMenuItems", () => {
  /* The wording is the live session menu's, verbatim: a card whose action reads
     differently from the button beside it is a bug nobody reports. */
  it("offers continue and delete", () => {
    expect(actions().map((item) => item.label)).toEqual([
      "Continue in a New Session…",
      "Delete Transcript…",
    ]);
  });

  it("greys Continue for a run outside a tracked checkout, with the reason", () => {
    const [cont] = actions({ workspace_id: null });
    expect(cont.disabled).toBe(true);
    expect(cont.detail).toContain("outside a checkout");
    expect(handoffBlocked(session())).toBeNull();
  });

  it("greys Delete for a run in a store shared with other runs", () => {
    const [, del] = actions({ store: "SharedDatabase" });
    expect(del.disabled).toBe(true);
    expect(del.detail).toContain("shared database");
    expect(actions({ store: "File" })[1].disabled).toBe(false);
  });
});
