import { describe, expect, it } from "vitest";
import { offersTerminalEditor } from "./terminalAction";

describe("the terminal editor action", () => {
  it("the_terminal_editor_action_is_hidden_until_the_flag_is_on", () => {
    expect(offersTerminalEditor(false, null)).toBe(false);
    expect(offersTerminalEditor(true, null)).toBe(true);
  });

  it("stays hidden for a file the editor could not open anyway", () => {
    // The daemon refuses an editor session for both of these (R5), so the
    // action would offer a buffer that arrives as a refusal.
    expect(offersTerminalEditor(true, "Binary file.")).toBe(false);
    expect(offersTerminalEditor(true, "Too large to open.")).toBe(false);
    expect(offersTerminalEditor(false, "Binary file.")).toBe(false);
  });
});
