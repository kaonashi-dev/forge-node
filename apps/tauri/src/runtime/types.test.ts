import { describe, expect, it } from "vitest";
import { sessionFixture } from "./sessions.fixture";
import { sessionIsActive, sessionTitle, type Session } from "./types";

const session = (partial: Partial<Session>): Session => sessionFixture(partial);

describe("session display helpers", () => {
  it("prefers the user title, then OSC, then kind", () => {
    expect(sessionTitle(session({ title: { user: "mine", terminal: "osc" } }))).toBe("mine");
    expect(sessionTitle(session({ title: { user: null, terminal: "osc" } }))).toBe("osc");
    expect(sessionTitle(session({ title: { user: null, terminal: null } }))).toBe("Shell");
    expect(
      sessionTitle(
        session({
          kind: "Agent",
          agent_provider_id: "claude",
          title: { user: null, terminal: null },
        }),
      ),
    ).toBe("claude");
  });

  it("treats empty or whitespace OSC titles as unset", () => {
    expect(
      sessionTitle(
        session({
          kind: "Agent",
          agent_provider_id: "cursor",
          title: { user: null, terminal: "" },
        }),
      ),
    ).toBe("cursor");
    expect(
      sessionTitle(
        session({
          kind: "Agent",
          agent_provider_id: "cursor",
          title: { user: "  ", terminal: "   " },
        }),
      ),
    ).toBe("cursor");
  });

  it("treats Starting and Running as active", () => {
    expect(sessionIsActive("Starting")).toBe(true);
    expect(sessionIsActive("Running")).toBe(true);
    expect(sessionIsActive("Orphaned")).toBe(false);
  });
});
