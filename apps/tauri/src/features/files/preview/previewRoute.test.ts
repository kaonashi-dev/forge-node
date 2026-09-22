import { describe, expect, it } from "vitest";

import { hasEditableSource, openingSurface, opensInEditor, previewKindFor } from "./previewRoute";

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

  /* A raster has no source the editor will open. Markdown and SVG do. */
  it("treats markdown and svg as editable source", () => {
    for (const path of ["README.md", "docs/notes.markdown", "page.mdx", "logo.svg"]) {
      expect(hasEditableSource(path), path).toBe(true);
    }
    for (const path of ["shot.png", "src/main.rs", ".md"]) {
      expect(hasEditableSource(path), path).toBe(false);
    }
  });

  /* A line is a caret. The preview has none, so that open is the source.
     Clicking the file with no line still lands on the rendering. */
  it("opens editable source in the editor only when a line is requested", () => {
    expect(openingSurface("README.md")).toBe("preview");
    expect(openingSurface("docs/notes.markdown", 12)).toBe("editor");
    expect(openingSurface("logo.svg", 1)).toBe("editor");
    expect(openingSurface("shot.png", 4)).toBe("preview");
    expect(openingSurface("src/main.rs", 4)).toBe("editor");
  });
});
