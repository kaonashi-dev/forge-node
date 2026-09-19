import { describe, expect, it } from "vitest";
import { sessionFixture } from "./sessions.fixture";
import { sessionIsActive, sessionIsAgent, sessionTitle, type Session } from "./runtime";
import { sessionWork } from "../features/sessions/work";

const session = (partial: Partial<Session>): Session => sessionFixture(partial);

describe("an Editor session", () => {
  /*
   * R31: an editor is its own kind wherever the rail, the glyphs and the titles
   * read a session. Falling through to the shell branch would put a terminal
   * glyph and the word "Shell" on a file, and give it the green dot a shell
   * earns by sitting at a prompt — which an editor never does.
   *
   * The one surface it does *not* reach is the window's session strip: an open
   * file belongs to the Code strip, so `sessionsInWorkspace` keeps it out of
   * the terminal tabs (see sessionScope.test.ts) — Code is its surface.
   */
  it("an_editor_session_is_not_rendered_as_a_shell", () => {
    const editor = session({ kind: "Editor", title: { user: null, terminal: null } });

    expect(sessionTitle(editor)).toBe("Editor");
    expect(sessionTitle(editor)).not.toBe("Shell");
    // The glyph branches on the kind, so an editor must not read as an agent
    // either: it has no provider and is not one.
    expect(sessionIsAgent(editor)).toBe(false);
    expect(sessionWork(editor, { wants_you: false }, Date.now())).toBe("idle");
    expect(sessionWork(editor, { wants_you: false }, Date.now())).not.toBe("running");

    // The OSC title the editor sets still wins, as it does for every kind:
    // the fallback is what a session with nothing to say is called.
    const named = session({ kind: "Editor", title: { user: null, terminal: "main.rs" } });
    expect(sessionTitle(named)).toBe("main.rs");
  });
});

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
    expect(
      sessionTitle(
        session({
          kind: "Editor",
          title: { user: null, terminal: null },
        }),
      ),
    ).toBe("Editor");
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
