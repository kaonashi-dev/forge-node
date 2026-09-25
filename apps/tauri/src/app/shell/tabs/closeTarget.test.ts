import { describe, expect, it } from "vitest";
import { closeTarget } from "./closeTarget";
import { TERMINAL_VIEW, type ParkedViews } from "../../../navigation/views";

const empty: ParkedViews = { open: [], active: TERMINAL_VIEW };

describe("closeTarget", () => {
  it("closes the session while the terminal is up", () => {
    expect(closeTarget("session", empty)).toEqual({ kind: "session" });
  });

  it("closes the open file rather than the session it came from", () => {
    const views: ParkedViews = {
      open: [{ kind: "editor-terminal", session: "s-1", path: "src/main.rs" }],
      active: { kind: "editor-terminal", session: "s-1", path: "src/main.rs" },
    };
    expect(closeTarget("code", views)).toEqual({
      kind: "view",
      view: { kind: "editor-terminal", session: "s-1", path: "src/main.rs" },
    });
  });

  it("closes only the active file when several are open", () => {
    const views: ParkedViews = {
      open: [
        { kind: "editor-terminal", session: "s-1", path: "a.rs" },
        { kind: "diff" },
        { kind: "editor-terminal", session: "s-1", path: "b.rs" },
      ],
      active: { kind: "diff" },
    };
    expect(closeTarget("code", views)).toEqual({ kind: "view", view: { kind: "diff" } });
  });

  // The session tab is selected, so the file behind it is not what "close"
  // is pointing at — even though it is still parked.
  it("closes the session when the terminal is showing over parked views", () => {
    const views: ParkedViews = {
      open: [{ kind: "editor-terminal", session: "s-1", path: "a.rs" }],
      active: { kind: "editor-terminal", session: "s-1", path: "a.rs" },
    };
    expect(closeTarget("session", views)).toEqual({ kind: "session" });
  });

  it("leaves background sessions alone when Code is empty", () => {
    expect(closeTarget("code", empty)).toEqual({ kind: "none" });
  });

  it("joins the extra column when that pane holds the keyboard", () => {
    expect(
      closeTarget("session", empty, { kind: "session", extra: "s-2", focused: "extra" }),
    ).toEqual({ kind: "unsplit" });
    expect(closeTarget("code", empty, { kind: "code", extra: null, focused: "extra" })).toEqual({
      kind: "unsplit",
    });
  });
});
