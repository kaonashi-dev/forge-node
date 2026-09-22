import { describe, expect, it } from "vitest";
import { sessionKindWord, viewKindWord } from "./switchKind";

describe("switcher kind words", () => {
  it("names an editor pane by what it is, not by its file", () => {
    expect(viewKindWord({ kind: "editor-terminal", session: "s", path: "src/a.ts" })).toBe(
      "editor",
    );
    expect(viewKindWord({ kind: "pr_review", key: "o/r#42" })).toBe("review");
  });

  it("tells an agent session from a shell", () => {
    expect(sessionKindWord(true)).toBe("session");
    expect(sessionKindWord(false)).toBe("terminal");
  });
});
