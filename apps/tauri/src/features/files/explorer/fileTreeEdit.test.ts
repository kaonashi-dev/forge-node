import { describe, expect, it } from "vitest";
import { editedName, nameSelection } from "@forge-node/file-workbench";

/*
 * The two pure rules behind the Files panel's inline rename. They live in the
 * package because the field does, and they are imported here rather than
 * re-implemented: a second copy of "is this a cancel" would drift from the one
 * the input actually runs.
 */

describe("nameSelection", () => {
  it("preselects the stem and leaves the extension alone", () => {
    expect(nameSelection("README.md", true)).toEqual({ start: 0, end: 6 });
  });

  it("stops at the last dot, so a double extension keeps only its tail", () => {
    expect(nameSelection("archive.tar.gz", true)).toEqual({ start: 0, end: 11 });
  });

  it("takes a dotfile whole: a leading dot does not start an extension", () => {
    expect(nameSelection(".gitignore", true)).toEqual({ start: 0, end: 10 });
  });

  it("takes a name with no dot whole", () => {
    expect(nameSelection("Makefile", true)).toEqual({ start: 0, end: 8 });
  });

  it("takes a directory whole even when its name has a dot", () => {
    expect(nameSelection("src.old", false)).toEqual({ start: 0, end: 7 });
  });
});

describe("editedName", () => {
  it("passes a real change through, trimmed", () => {
    expect(editedName("  notes.md  ", "README.md")).toBe("notes.md");
  });

  it("cancels on a name that did not change", () => {
    expect(editedName("README.md", "README.md")).toBeNull();
  });

  it("cancels on empty and on whitespace alone", () => {
    expect(editedName("", "README.md")).toBeNull();
    expect(editedName("   ", "README.md")).toBeNull();
    expect(editedName("\t", "")).toBeNull();
  });

  it("accepts any non-blank name for a create, whose original is empty", () => {
    expect(editedName("main.rs", "")).toBe("main.rs");
  });

  // Trimming is what makes this a cancel: the typed value differs from the
  // original, and only the trimmed one is the same name.
  it("cancels when the only edit was surrounding whitespace", () => {
    expect(editedName(" README.md ", "README.md")).toBeNull();
  });
});
