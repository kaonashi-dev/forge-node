import { describe, expect, it } from "vitest";
import type { Session } from "./types";
import {
  FIXTURE_NAMES,
  request_create_editor_session,
  response_snapshot_populated,
  session_editor,
} from "./generated/fixtures";

describe("generated fixtures", () => {
  it("lists every JSON fixture", () => {
    expect(FIXTURE_NAMES.length).toBeGreaterThan(50);
  });

  it("includes a populated snapshot with usage meters", () => {
    const snapshot = response_snapshot_populated.Snapshot;
    expect(snapshot.usage.length).toBeGreaterThan(0);
    expect(snapshot.sessions.length).toBeGreaterThan(0);
  });

  /*
   * The fixtures are the daemon's own encoding of these shapes: they are what
   * catches a wire field the GUI's hand-written types drifted from. An editor
   * session and its request only do that once they are actually exported, and
   * the exporter is only re-run by hand.
   */
  it("fixtures_cover_the_editor_session", () => {
    expect(FIXTURE_NAMES).toContain("session_editor.json");
    expect(FIXTURE_NAMES).toContain("request_create_editor_session.json");

    // Assigning to `Session` is the check: a field the daemon renamed or
    // dropped fails the type check, not just this assertion.
    const session: Session = session_editor;
    expect(session.kind).toBe("Editor");
    expect(session.editor).toEqual({
      path: "src/main.rs",
      line: 12,
      column: 4,
      dirty: false,
      read_only: true,
      document_version: 1,
    });

    const request = request_create_editor_session.CreateEditorSession;
    expect(request.path).toBe("src/main.rs");
    expect(request.read_only).toBe(true);
  });
});
