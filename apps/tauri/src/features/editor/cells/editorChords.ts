import type { KeyPress } from "../../../contracts/terminal";

// ⌘G means find-next on macOS; the editor's Ctrl-G means go-to-line.
const PLAIN: Record<string, string> = {
  s: "s",
  c: "c",
  x: "x",
  f: "f",
  g: "n",
  a: "a",
  z: "z",
};

const SHIFTED: Record<string, string> = {
  g: "b",
  z: "y",
};

const ALTED: Record<string, string> = {
  f: "r",
  l: "g",
};

/** Unclaimed chords keep their native default, including ⌘V's paste event. */
export function editorKeyForChord(event: {
  key: string;
  code?: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing?: boolean;
  keyCode?: number;
}): KeyPress | null {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229 || event.ctrlKey)
    return null;
  if (!event.metaKey && !event.altKey) return null;
  const key = event.key.toLowerCase();
  // Option changes event.key to characters such as ÷ on macOS.
  const altSlash =
    event.altKey && !event.metaKey && (key === "/" || (!event.shiftKey && event.code === "Slash"));
  const metaSlash =
    event.metaKey && !event.altKey && (key === "/" || (event.shiftKey && key === "?"));
  // Ctrl-_ survives legacy PTY encoding; Ctrl-/ has no portable control byte.
  if (altSlash || metaSlash) {
    return { key: "_", ctrl: true, alt: false, shift: false };
  }
  if (!event.metaKey) return null;
  const table = event.altKey ? ALTED : event.shiftKey ? SHIFTED : PLAIN;
  if (event.altKey && event.shiftKey) return null;
  const mapped = table[key];
  if (!mapped) return null;
  return { key: mapped, ctrl: true, alt: false, shift: false };
}
