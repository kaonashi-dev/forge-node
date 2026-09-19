// Browser input translated for the shared editor command table.
import { editorKeyForMeta } from "../cells/editorChords";
import type { EditorInputEvent, EditorKey } from "../../../contracts/editor";

/** Mirrors `domain::editor_modifiers`; a bitfield, as every event source reports. */
export const MOD_SHIFT = 1;
export const MOD_CONTROL = 2;
export const MOD_ALT = 4;
export const MOD_META = 8;

const NAMED: Readonly<Record<string, EditorKey>> = {
  Enter: "Enter",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Delete",
  Escape: "Escape",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  ArrowUp: "Up",
  ArrowDown: "Down",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  Insert: "Insert",
};

export function modifiersOf(event: {
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}): number {
  return (
    (event.shiftKey ? MOD_SHIFT : 0) |
    (event.ctrlKey ? MOD_CONTROL : 0) |
    (event.altKey ? MOD_ALT : 0) |
    (event.metaKey ? MOD_META : 0)
  );
}

/**
 * The key an event names, or `null` when it names none this host understands.
 *
 * `null` is also the answer for a bare modifier press and for an unresolved
 * IME composition: the committed text arrives as its own `Text` event, and a
 * key synthesised mid-composition would insert the half the person is still
 * choosing between.
 */
export function keyOf(event: KeyboardEvent): EditorKey | null {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return null;
  const named = NAMED[event.key];
  if (named !== undefined) return named;
  const fn = /^F([1-9]|1[0-2])$/.exec(event.key);
  if (fn !== null) return { Function: Number(fn[1]) };
  // A single code point, which is what `key` is for a printable key. Longer
  // means a name this build does not know ("AudioVolumeUp"), and a name is
  // never a character to insert.
  const points = Array.from(event.key);
  if (points.length === 1) return { Char: points[0] };
  return null;
}

/** One key press as the editor's input, or `null` when it carries nothing. */
export function inputFor(event: KeyboardEvent): EditorInputEvent | null {
  const key = keyOf(event);
  if (key === null) return null;
  const chord = editorKeyForMeta(event);
  if (chord) return { Key: { key: { Char: chord.key }, modifiers: MOD_CONTROL } };
  if (event.metaKey) return null;
  return { Key: { key, modifiers: modifiersOf(event) } };
}

/** Committed text — a paste, or what an IME composition resolved to. */
export function textInput(text: string): EditorInputEvent {
  return { Text: text };
}
