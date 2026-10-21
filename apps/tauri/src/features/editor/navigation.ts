import { registerAction } from "../../actions/dispatch";
import type { EditorInputEvent } from "../../contracts/editor";
import type { KeyPress } from "../../contracts/terminal";
import { MOD_ALT } from "./dom/editorKeys";

/**
 * The editor's own keys for the two navigation actions.
 *
 * F12 and Alt-O rather than Alt-D: on macOS Option turns `d` into `∂` before
 * the DOM surface sees it, and a function key survives both surfaces unchanged.
 */
export const DEFINITION_KEY: KeyPress = { key: "F12", ctrl: false, alt: false, shift: false };
export const BACK_KEY: KeyPress = { key: "o", ctrl: false, alt: true, shift: false };

export const DEFINITION_INPUT: EditorInputEvent = {
  Key: { key: { Function: 12 }, modifiers: 0 },
};
export const BACK_INPUT: EditorInputEvent = { Key: { key: { Char: "o" }, modifiers: MOD_ALT } };

/**
 * Answer go-to-definition and go-back for the editor that has the keyboard.
 *
 * Registered on focus rather than on mount: two editor panes can be mounted at
 * once, and the most recent registration wins, so a mount-time handler would
 * send the key to whichever pane mounted last.
 */
export function bindEditorNavigation(send: (which: "definition" | "back") => void): () => void {
  const unbind = [
    registerAction("go_to_definition", () => send("definition")),
    registerAction("go_back", () => send("back")),
  ];
  return () => {
    for (const release of unbind) release();
  };
}
