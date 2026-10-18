import { describe, expect, it } from "vitest";
import { clipboardPaste, type PasteTarget } from "./clipboard";

function data(
  types: string[],
  text = "",
  files: string[] = [],
  uri = "",
): Pick<DataTransfer, "getData" | "types"> & { items: DataTransfer["items"] } {
  return {
    types,
    getData: (format: string) =>
      format === "text/plain" ? text : format === "text/uri-list" ? uri : "",
    items: files.map((type) => ({ kind: "file", type })) as unknown as DataTransfer["items"],
  };
}

const TARGETS: PasteTarget[] = ["editor", "shell", "agent"];

describe("clipboard paste routing", () => {
  it("preserves multiline text for bracketed paste at every destination", () => {
    for (const target of TARGETS) {
      expect(clipboardPaste(data(["text/plain"], "first\nsecond"), target)).toEqual({
        kind: "text",
        text: "first\nsecond",
      });
    }
  });

  it("pastes the text half of an image clipboard into an editor or a shell", () => {
    // Notes and Excel copy a rendering beside the text; only an agent can use
    // the rendering, and only by reading the system clipboard itself.
    for (const target of ["editor", "shell"] as const) {
      expect(
        clipboardPaste(data(["image/png", "text/plain"], "quarterly numbers"), target),
      ).toEqual({
        kind: "text",
        text: "quarterly numbers",
      });
    }
  });

  it("hands an image to an agent, which reads the clipboard itself", () => {
    expect(clipboardPaste(data(["image/png"], ""), "agent")).toEqual({ kind: "agent" });
    expect(clipboardPaste(data(["image/png", "text/plain"], "source url"), "agent")).toEqual({
      kind: "agent",
    });
  });

  it("never asks a pane to press ctrl-v without an image", () => {
    for (const target of TARGETS) {
      expect(clipboardPaste(data(["image/png"], ""), target)).toEqual(
        target === "agent" ? { kind: "agent" } : { kind: "empty" },
      );
      expect(clipboardPaste(data(["Files"], ""), target)).toEqual({ kind: "empty" });
    }
  });

  it("keeps a path copied from the Finder as text", () => {
    // Finder puts `Files` beside the path text. Pressing ctrl-v for it gave
    // the agent nothing; the path is what the paste should carry.
    for (const target of TARGETS) {
      expect(clipboardPaste(data(["Files", "text/plain"], "/Users/me/notes.md"), target)).toEqual({
        kind: "text",
        text: "/Users/me/notes.md",
      });
    }
  });

  it("hands a screenshot to an agent the way WebKit reports it: a file, not a type", () => {
    expect(clipboardPaste(data(["Files"], "", ["image/png"]), "agent")).toEqual({ kind: "agent" });
    expect(clipboardPaste(data(["Files", "text/html"], "", ["image/jpeg"]), "agent")).toEqual({
      kind: "agent",
    });
    for (const target of ["editor", "shell"] as const) {
      expect(clipboardPaste(data(["Files"], "", ["image/png"]), target)).toEqual({ kind: "empty" });
    }
  });

  it("pastes the path of an image file copied in the Finder, not the image", () => {
    const finder = data(
      ["Files", "text/plain", "text/uri-list"],
      "/Users/me/shot.png",
      ["image/png"],
      "file:///Users/me/shot.png",
    );
    expect(clipboardPaste(finder, "agent")).toEqual({ kind: "text", text: "/Users/me/shot.png" });
  });

  it("does not send control keys for empty or unsupported clipboard data", () => {
    for (const target of TARGETS) {
      expect(clipboardPaste(null, target)).toEqual({ kind: "empty" });
      expect(clipboardPaste(data(["text/html"]), target)).toEqual({ kind: "empty" });
    }
  });
});
