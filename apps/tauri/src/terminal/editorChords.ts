// The platform's edit chords, delivered as the keys `forge-editor` reads.
//
// The editor owns its key table and saves/copies/cuts on Ctrl-S/C/X; a Mac
// keyboard sends those gestures as ⌘. Paste is deliberately absent: the
// WebView's `paste` event carries the clipboard into the pane, and
// `navigator.clipboard.readText` is refused outside a gesture (`PASTE_CHORD`
// in `actions/keys.ts`).

import type { KeyPress } from "../runtime/api";

/**
 * ⌘ chords the editor owns, as the key its own table reads.
 *
 * `⌘G` and `⇧⌘G` are the platform's find-next / find-previous and land on the
 * editor's `Ctrl-N` / `Ctrl-B`, not on a second `Ctrl-G`: that chord is "go to
 * line" in the TUI, which is what `⌥⌘L` reaches instead. Documented in
 * `docs/editor.md`, because this is the one table where the two keymaps differ.
 */
const PLAIN: Record<string, string> = {
  s: "s",
  c: "c",
  x: "x",
  f: "f",
  g: "n",
  a: "a",
  z: "z",
};

/** ⇧⌘ chords. `⇧⌘Z` is redo, the platform's spelling of `Ctrl-Y`. */
const SHIFTED: Record<string, string> = {
  g: "b",
  z: "y",
};

/** ⌥⌘ chords, for the two the plain row could not hold. */
const ALTED: Record<string, string> = {
  f: "r",
  l: "g",
};

/**
 * The editor key a platform chord stands for, or `null` when the chord is the
 * platform's own.
 *
 * A chord this does not claim keeps its default, which is what lets ⌘V reach
 * the WebView's `paste` event and ⌘W still close the tab.
 */
export function editorKeyForMeta(event: {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): KeyPress | null {
  if (!event.metaKey || event.ctrlKey) return null;
  const key = event.key.toLowerCase();
  // Ctrl-_ survives legacy PTY encoding; Ctrl-/ has no portable control byte.
  if (!event.altKey && (key === "/" || (event.shiftKey && key === "?"))) {
    return { key: "_", ctrl: true, alt: false, shift: false };
  }
  const table = event.altKey ? ALTED : event.shiftKey ? SHIFTED : PLAIN;
  if (event.altKey && event.shiftKey) return null;
  const mapped = table[key];
  if (!mapped) return null;
  return { key: mapped, ctrl: true, alt: false, shift: false };
}
