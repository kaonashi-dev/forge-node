import { describe, expect, it } from "vitest";

/*
 * Read at transform time, like `editorPane.test.ts` does for the cell pane:
 * these are claims about what the surface is wired to that no type check can
 * make, and the vitest environment is `node`, so there is nothing to render
 * `EditorView` into either.
 */
const VIEW = import.meta.glob("./EditorView.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const CENTER_STACK = import.meta.glob("../../../app/shell/CenterStack.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const view = () => rawSource(VIEW, "EditorView.tsx");
const centerStack = () => rawSource(CENTER_STACK, "CenterStack.tsx");

function rawSource(sources: Record<string, string>, name: string): string {
  const source = Object.values(sources)[0];
  if (source === undefined) throw new Error(`the raw-source glob matched no ${name}`);
  return source;
}

describe("the DOM editor surface", () => {
  /* Both panes serve one `editor-terminal` view; `[editor] surface` picks
     between them, and it arrives on the handshake because the Code region has
     to know which to build before an editor session exists. */
  it("enters the editor keymap for as long as the surface is on screen", () => {
    expect(view()).toContain("enterContext(EDITOR)");
  });

  it("reads the editor font preference rather than the chrome type scale", () => {
    expect(view()).toContain("EDITOR_FONT_SIZE_KEY");
    expect(view()).toContain("--ed-font-size");
  });

  it("is the other half of the editor view, chosen by the daemon", () => {
    expect(centerStack()).toContain("<EditorView");
    expect(centerStack()).toContain("<EditorTerminalPane");
    expect(centerStack()).toContain('editorSurface === "dom"');
  });

  /* The whole point of B: no canvas, no cell grid, no second emulator. A
     surface that reached for the renderer would be the old path wearing a new
     name. */
  it("carries no cell machinery", () => {
    for (const absent of [
      "TerminalRenderer",
      "Viewport",
      "CellsPayload",
      "<canvas",
      "editorCellsChannel",
    ]) {
      expect(view(), `the surface must not use ${absent}`).not.toContain(absent);
    }
  });

  /* Rows are positioned by their line number inside a container as tall as the
     file, so the browser composites the scroll and a new window never moves
     what is on screen. */
  it("positions rows by line number in a full-height container", () => {
    expect(view()).toContain("data-line={row.line}");
    expect(view()).toContain("row.line * lineHeight");
    expect(view()).toContain("total_lines");
  });

  /* One view request per animation frame, never one per scroll event: a wheel
     burst fires dozens and the browser has already moved the content for all
     of them. */
  it("throttles the view request to an animation frame", () => {
    expect(view()).toContain("requestAnimationFrame");
    expect(view()).toContain("setEditorSurfaceView");
    expect(view()).toContain("sameWindow");
  });

  /* Input is named and batched: there is no PTY to encode an escape sequence
     for, and a key repeat is one call rather than one per repeat. */
  it("sends named keys in batches, and committed text apart from them", () => {
    expect(view()).toContain("sendEditorSurfaceInput");
    expect(view()).toContain("inputFor");
    expect(view()).toContain("onCompositionEnd");
    expect(view()).toContain("textInput");
    expect(view()).not.toContain("sendEditorKey");
  });

  /* A coalesced burst can put two frames on the socket out of the order they
     were built in; the older one has nothing the newer one is missing. */
  it("drops a frame older than the one on screen", () => {
    expect(view()).toContain("doc_version < shownVersion");
  });

  /* The surface *is* the accessibility tree now — a real multiline textbox,
     not a canvas with a hidden paragraph beside it. */
  it("is a real textbox rather than a mirror of one", () => {
    expect(view()).toContain('role="textbox"');
    expect(view()).toContain('aria-multiline="true"');
    expect(view()).not.toContain('role="application"');
  });

  /* Every colour comes from the theme's own ANSI ramp, the same one the cell
     surface resolves scopes through. A hex here would be a second theme. */
  it("takes every scope colour from the theme", () => {
    expect(view()).toContain("var(--forge-ansi-");
    expect(view()).not.toMatch(/#[0-9a-fA-F]{6}/);
  });

  /* A band under the rows, measured the same way the caret is: a selection
     computed from a column would drift past a wide character or a ligature. */
  it("paints the selection the host published", () => {
    expect(view()).toContain("measureRange");
    expect(view()).toContain("selectionRects");
    expect(view()).toContain("current.selection.flatMap");
  });

  /* Every column on this wire is UTF-16, in both directions — including the
     one a click reports. The browser is asked where the point landed rather
     than the surface deriving it from an x. */
  it("reports a pointer in the same units the frame uses", () => {
    expect(view()).toContain("caretOffsetAt");
    expect(view()).toContain("caretPositionFromPoint");
    expect(view()).toContain("caretRangeFromPoint");
    expect(view()).toContain('kind: "Down"');
    expect(view()).toContain('kind: "Drag"');
  });

  /* The draft and the other write both still exist, so the three answers are
     the same three the cell pane offers. */
  it("keeps the conflict's three answers", () => {
    expect(view()).toContain("overwriteEditorBuffer");
    expect(view()).toContain("reloadEditorBuffer");
    expect(view()).toContain("CompareView");
  });
});
