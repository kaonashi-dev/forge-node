import { describe, expect, it } from "vitest";

/*
 * Read at transform time, like `langIcons.test.ts` reads the icon set: this
 * app has no `node:fs`, and these are claims about what the components are
 * wired to that no type check can make. The vitest environment is `node`, so
 * there is nothing to render them into either.
 */
const SOURCE = import.meta.glob("./EditorTerminalPane.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const CENTER_STACK = import.meta.glob("../shell/CenterStack.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const pane = () => Object.values(SOURCE)[0] ?? "";
const centerStack = () => Object.values(CENTER_STACK)[0] ?? "";

describe("the terminal editor pane", () => {
  /*
   * R28: the `editor-terminal` view is the terminal pane, in `CenterStack`'s
   * `.center-view` branch, on the renderer and palette the initial bundle
   * already carries — not a second engine and not the DOM editor.
   */
  it("the_editor_view_renders_a_terminal_pane", () => {
    expect(centerStack()).toContain('active().kind === "editor-terminal"');
    expect(centerStack()).toContain("<EditorTerminalPane");

    for (const shared of ["./renderer", "./palette", "./metrics", "./viewport"]) {
      expect(pane(), `the pane reuses ${shared}`).toContain(`from "${shared}"`);
    }
    // Input goes through the same textarea encoding path as `TerminalPane`:
    // keys, IME composition and paste, never a blind Cmd→Ctrl translation.
    expect(pane()).toContain("onCompositionEnd");
    expect(pane()).toContain("sendEditorKey");
    expect(pane()).toContain("sendEditorPaste");
    expect(pane()).toContain("resizeEditor");
  });

  /*
   * R30: H1 writes nothing to the checkout, so the pane must not carry the DOM
   * editor's Save or autosave. A button here would be one the editor refuses
   * (`integrated save is not available`), which is worse than no button.
   */
  it("the_terminal_pane_has_no_save_action", () => {
    for (const absent of ["saveFile", "AUTOSAVE_KEY", "autosave", "Save"]) {
      expect(pane(), `the pane must not mention ${absent}`).not.toContain(absent);
    }
  });

  /*
   * The renderer paints the caret only while `focused` is set, and only the
   * textarea's focus can set it: without this wiring the editor's caret is
   * never drawn even though the engine reports it on every frame.
   */
  it("the_editor_pane_wires_focus_to_the_caret", () => {
    expect(pane()).toContain("renderer.focused = focused");
    expect(pane()).toContain("onFocus");
    expect(pane()).toContain("onBlur");
    expect(pane()).toContain("new CursorBlink(");
  });
});
