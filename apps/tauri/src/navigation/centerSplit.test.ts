import { describe, expect, it } from "vitest";
import {
  CLOSED_SPLIT,
  canSplitCenter,
  clampSplitRatio,
  viewAllowsTerminalSplit,
  visibleSplit,
  type CenterSplitState,
} from "./centerSplit";
import { TERMINAL_VIEW, type WorkbenchView } from "./views";

const file: WorkbenchView = {
  kind: "editor-terminal",
  session: "ed",
  path: "src/main.rs",
};
const preview: WorkbenchView = { kind: "preview", path: "README.md" };
const diff: WorkbenchView = { kind: "diff" };
const codeSplit: CenterSplitState = { kind: "code", extra: null, focused: "primary" };
const sessionSplit: CenterSplitState = { kind: "session", extra: "s-2", focused: "extra" };

describe("viewAllowsTerminalSplit", () => {
  it("allows a file or a rendered preview, not a diff", () => {
    expect(viewAllowsTerminalSplit(file)).toBe(true);
    expect(viewAllowsTerminalSplit(preview)).toBe(true);
    expect(viewAllowsTerminalSplit(diff)).toBe(false);
    expect(viewAllowsTerminalSplit(TERMINAL_VIEW)).toBe(false);
  });
});

describe("canSplitCenter", () => {
  it("always allows a session terminal", () => {
    expect(canSplitCenter("session", TERMINAL_VIEW)).toBe(true);
    expect(canSplitCenter("session", diff)).toBe(true);
  });

  it("allows Code only while a file or preview is on screen", () => {
    expect(canSplitCenter("code", file)).toBe(true);
    expect(canSplitCenter("code", preview)).toBe(true);
    expect(canSplitCenter("code", diff)).toBe(false);
    expect(canSplitCenter("code", { kind: "search" })).toBe(false);
  });
});

describe("visibleSplit", () => {
  it("hides a remembered split in Settings", () => {
    expect(visibleSplit(codeSplit, true, "code", file)).toBe("closed");
    expect(visibleSplit(sessionSplit, true, "session", TERMINAL_VIEW)).toBe("closed");
  });

  it("shows a file beside the session while Code is on a file", () => {
    expect(visibleSplit(codeSplit, false, "code", file)).toBe("code");
    expect(visibleSplit(codeSplit, false, "code", preview)).toBe("code");
  });

  it("keeps a diff full width without forgetting the split", () => {
    expect(visibleSplit(codeSplit, false, "code", diff)).toBe("closed");
    expect(visibleSplit(codeSplit, false, "session", TERMINAL_VIEW)).toBe("closed");
  });

  it("hides two terminals while Code is up, without converting them", () => {
    expect(visibleSplit(sessionSplit, false, "code", file)).toBe("closed");
    expect(visibleSplit(sessionSplit, false, "session", TERMINAL_VIEW)).toBe("session");
  });

  it("is closed when nothing was split", () => {
    expect(visibleSplit(CLOSED_SPLIT, false, "session", TERMINAL_VIEW)).toBe("closed");
  });
});

describe("clampSplitRatio", () => {
  it("keeps a usable column on either side", () => {
    expect(clampSplitRatio(0.5)).toBe(0.5);
    expect(clampSplitRatio(0)).toBe(0.2);
    expect(clampSplitRatio(1)).toBe(0.8);
    expect(clampSplitRatio(Number.NaN)).toBe(0.5);
  });
});
