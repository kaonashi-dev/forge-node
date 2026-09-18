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
const CENTER_STACK = import.meta.glob("../../../app/shell/CenterStack.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const pane = () => rawSource(SOURCE, "EditorTerminalPane.tsx");
const centerStack = () => rawSource(CENTER_STACK, "CenterStack.tsx");

function rawSource(sources: Record<string, string>, name: string): string {
  const source = Object.values(sources)[0];
  if (source === undefined) throw new Error(`the raw-source glob matched no ${name}`);
  return source;
}

describe("the terminal editor pane", () => {
  /*
   * R28: the `editor-terminal` view is the terminal pane, in `CenterStack`'s
   * `.center-view` branch, on the renderer and palette the initial bundle
   * already carries — not a second engine and not the DOM editor.
   */
  it("the_editor_view_renders_a_terminal_pane", () => {
    expect(centerStack()).toContain('active().kind === "editor-terminal"');
    expect(centerStack()).toContain("<EditorTerminalPane");

    for (const shared of [
      "../../../shared/cell-grid/renderer",
      "../../../shared/cell-grid/palette",
      "../../../shared/cell-grid/metrics",
      "../../../shared/cell-grid/viewport",
    ]) {
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
    expect(pane()).toContain("paintCaret");
  });

  /*
   * A canvas is unreadable to an accessibility tree, so the pane carries a
   * hidden mirror of the editor's own state — path, position, flags and the
   * caret's line — plus a polite live region for what the status row says.
   * `application` and not `textbox`: the keys go to the textarea, and a reader
   * that took the mirror for an input would offer editing keys against a node
   * that has none.
   */
  it("the_editor_pane_mirrors_its_state_for_a_screen_reader", () => {
    expect(pane()).toContain('role="application"');
    expect(pane()).toContain('aria-roledescription="code editor"');
    expect(pane()).toContain("aria-label={aria().label}");
    expect(pane()).toContain("aria-readonly={aria().readOnly}");
    expect(pane()).toContain('aria-live="polite"');
    expect(pane()).toContain("editorAria");
    expect(pane()).toContain("editorAnnouncement");
    // Everything it says comes from the session, never from the cell grid.
    expect(pane()).not.toContain("viewport.rows[");
  });

  /*
   * The platform's edit chords cannot reach the editor as keys: its save,
   * copy and cut are Ctrl-S/C/X, and a Mac keyboard sends the gestures as ⌘.
   * The pane translates them through `editorChords`; without that, `Cmd-S`
   * saves nothing and closing loses the draft.
   */
  it("the_editor_pane_delivers_the_platform_chords", () => {
    expect(pane()).toContain("editorKeyForMeta");
    expect(pane()).toContain("event.metaKey");
  });
});
