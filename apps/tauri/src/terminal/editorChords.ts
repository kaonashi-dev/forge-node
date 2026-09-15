// The platform's edit chords, delivered as the keys `forge-editor` reads.
//
// The editor owns its key table and saves/copies/cuts on Ctrl-S/C/X; a Mac
// keyboard sends those gestures as ⌘. Paste is deliberately absent: the
// WebView's `paste` event carries the clipboard into the pane, and
// `navigator.clipboard.readText` is refused outside a gesture (`PASTE_CHORD`
// in `actions/keys.ts`).

import type { KeyPress } from "../runtime/api";

/**
 * The editor key a platform chord stands for, or `null` when the chord is the
 * platform's own.
 */
export function editorKeyForMeta(event: {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): KeyPress | null {
  if (!event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return null;
  const key = event.key.toLowerCase();
  if (key !== "s" && key !== "c" && key !== "x") return null;
  return { key, ctrl: true, alt: false, shift: false };
}
