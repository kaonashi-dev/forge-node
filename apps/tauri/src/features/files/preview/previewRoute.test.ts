import { describe, expect, it } from "vitest";

import { opensInEditor, previewKindFor } from "./previewRoute";

describe("which surface a file opens on", () => {
  it("sends a rendered kind to the preview", () => {
    expect(previewKindFor("README.md")).toBe("markdown");
    expect(previewKindFor("docs/notes.markdown")).toBe("markdown");
    expect(previewKindFor("a/b/logo.svg")).toBe("svg");
    expect(previewKindFor("assets/shot.PNG")).toBe("image");
    expect(previewKindFor("a.jpeg")).toBe("image");
  });

  /* Text is the common answer and the right default: the editor is what every
     file the grid can show opens in. */
  it("sends everything else to the editor", () => {
    for (const path of ["src/main.rs", "Cargo.toml", "Makefile", "a/b/.gitignore", "noext"]) {
      expect(previewKindFor(path), path).toBeNull();
      expect(opensInEditor(path), path).toBe(true);
    }
  });

  /* A query or a fragment is part of a Markdown `src`, not of the name. */
  it("ignores a query and a fragment", () => {
    expect(previewKindFor("logo.svg?v=2")).toBe("svg");
    expect(previewKindFor("shot.png#top")).toBe("image");
  });

  /* A dotfile is not an extension: `.gitignore` is a name that starts with a
     dot, and reading `gitignore` as its type would be a guess. */
  it("does not read a dotfile's name as an extension", () => {
    expect(previewKindFor(".md")).toBeNull();
    expect(previewKindFor("a/.svg")).toBeNull();
  });
});
