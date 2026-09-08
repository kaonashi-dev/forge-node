import { describe, expect, it } from "vitest";
import { clipboardPaste } from "./clipboard";

describe("terminal clipboard routing", () => {
  it("preserves multiline text for bracketed paste", () => {
    expect(clipboardPaste({ types: ["text/plain"], getData: () => "first\nsecond" })).toEqual({
      kind: "text",
      text: "first\nsecond",
    });
  });

  it("delegates image attachments even when they include a source URL", () => {
    for (const type of ["Files", "image/png"]) {
      expect(
        clipboardPaste({ types: [type, "text/plain"], getData: () => "https://example.com" }),
      ).toEqual({ kind: "agent" });
    }
  });

  it("does not send control keys for empty or unsupported clipboard data", () => {
    expect(clipboardPaste(null)).toEqual({ kind: "empty" });
    expect(clipboardPaste({ types: ["text/html"], getData: () => "" })).toEqual({ kind: "empty" });
  });
});
