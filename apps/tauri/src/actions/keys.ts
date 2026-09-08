// Keystroke specs, in `theme tokens::actions`' own vocabulary.
//
// A chord is written the way `actions.rs` writes it — `cmd-shift-o`,
// `ctrl-tab`, `down` — so the two tables can be read side by side and a
// divergence is visible rather than inferred.
//
// Matching prefers `KeyboardEvent.code` over `.key`, which is what makes a
// chord land on the physical key it was drawn on. `cmd-shift-[` produces `{`
// on a US layout and something else again on a Latin one; the position does
// not move, and neither should the shortcut.

export type Chord = {
  /** Lower-case key name, as written in the spec. */
  key: string;
  /** `KeyboardEvent.code` this key sits at on a US layout, when it has one. */
  code: string | null;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
};

/** `cmd` on macOS, `ctrl` everywhere else — `actions::MOD`. */
export const MOD: "cmd" | "ctrl" = isMac() ? "cmd" : "ctrl";

/**
 * Copy and paste keep the platform's own habit: `cmd-c`/`cmd-v` on macOS,
 * where no terminal wants those bytes, and `ctrl-shift-c`/`ctrl-shift-v`
 * elsewhere, where the unshifted pair belongs to the PTY.
 */
export const CLIPBOARD_MOD: string = isMac() ? "cmd" : "ctrl-shift";

/**
 * The chord the *keymap* answers paste on, or `null` where the platform does.
 *
 * `cmd-v` on macOS is not ours to take. Claiming it made the dispatcher call
 * `preventDefault()` in the capture phase, which is exactly what stops the
 * WebView from firing the `paste` event the terminal pane already handles —
 * and the fallback it fell back to, `navigator.clipboard.readText()`, is
 * refused by WKWebView outside a paste gesture and returns nothing. The result
 * was a chord that looked bound and pasted nothing, in agent sessions most
 * visibly, because that is where people paste.
 *
 * Leaving it unbound hands `cmd-v` to the platform, which delivers a real
 * `paste` event to the hidden textarea with the text already on it.
 * `ctrl-shift-v` elsewhere is nobody's native chord, so there it stays ours.
 */
export const PASTE_CHORD: string | null = isMac() ? null : "ctrl-shift-v";

/**
 * Whether this event is the platform's select-all chord.
 *
 * Same reason `PASTE_CHORD` is null on macOS: if the dispatcher claims it
 * and `preventDefault()`s, the field never selects. Matched on `key` rather
 * than `code` so a Latin layout's physical `KeyQ` that types `a` still
 * selects — that is the character the OS binds, not the US keycap.
 */
export function isSelectAll(event: KeyboardEvent): boolean {
  if (event.altKey || event.shiftKey) return false;
  if (event.key.toLowerCase() !== "a") return false;
  return isMac() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
}

export function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
}

const NAMED_CODES: Record<string, string> = {
  tab: "Tab",
  enter: "Enter",
  escape: "Escape",
  space: "Space",
  backspace: "Backspace",
  delete: "Delete",
  up: "ArrowUp",
  down: "ArrowDown",
  left: "ArrowLeft",
  right: "ArrowRight",
  home: "Home",
  end: "End",
  pageup: "PageUp",
  pagedown: "PageDown",
  ",": "Comma",
  ".": "Period",
  "/": "Slash",
  ";": "Semicolon",
  "'": "Quote",
  "[": "BracketLeft",
  "]": "BracketRight",
  "\\": "Backslash",
  "-": "Minus",
  "=": "Equal",
  "`": "Backquote",
};

const NAMED_KEYS: Record<string, string> = {
  up: "arrowup",
  down: "arrowdown",
  left: "arrowleft",
  right: "arrowright",
};

/** Parse `"cmd-shift-o"`. Throws on an unparseable spec, like `KeyBinding::new`. */
export function parseChord(spec: string): Chord {
  const tokens = spec.split("-");
  const chord: Chord = {
    key: "",
    code: null,
    ctrl: false,
    alt: false,
    shift: false,
    meta: false,
  };

  // Modifiers are taken from the left while they keep being modifiers; the
  // rest is the key. Written this way so `cmd--` is `cmd` plus the minus key
  // rather than a parse error.
  let index = 0;
  while (index < tokens.length - 1 && MODIFIERS.has(tokens[index])) {
    switch (tokens[index]) {
      case "cmd":
        chord.meta = true;
        break;
      case "ctrl":
        chord.ctrl = true;
        break;
      case "alt":
        chord.alt = true;
        break;
      default:
        chord.shift = true;
    }
    index += 1;
  }

  const raw = tokens.slice(index).join("-") || "-";
  if (raw === "") throw new Error(`no key in "${spec}"`);
  if (MODIFIERS.has(raw)) throw new Error(`"${spec}" is modifiers with no key`);
  chord.key = raw.toLowerCase();
  chord.code = codeFor(chord.key);
  return chord;
}

const MODIFIERS = new Set(["cmd", "ctrl", "alt", "shift"]);

/**
 * Whether a key name is one a `keydown` can actually produce.
 *
 * `parseChord` takes anything after the modifiers as the key, so
 * `"not-a-real-chord"` parses cleanly into a binding that can never fire.
 * That is fine for the table in this file, which is written by hand and read
 * by a test — but not for §4.1's user keymap, where the string comes from
 * storage and a dead binding would look like a shortcut that simply broke.
 */
export function keyIsKnown(key: string): boolean {
  const lower = key.toLowerCase();
  if (lower.length === 1) return true;
  if (/^f([1-9]|1[0-9]|2[0-4])$/.test(lower)) return true;
  return lower in NAMED_CODES;
}

function codeFor(key: string): string | null {
  if (NAMED_CODES[key]) return NAMED_CODES[key];
  if (/^[a-z]$/.test(key)) return `Key${key.toUpperCase()}`;
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  return null;
}

/**
 * Whether an event is this chord.
 *
 * The modifier comparison is exact: `cmd-k` must not fire on `cmd-shift-k`,
 * or a chord would swallow the one drawn beside it.
 */
export function matches(chord: Chord, event: KeyboardEvent): boolean {
  if (event.ctrlKey !== chord.ctrl) return false;
  if (event.altKey !== chord.alt) return false;
  if (event.shiftKey !== chord.shift) return false;
  if (event.metaKey !== chord.meta) return false;
  if (chord.code && event.code === chord.code) return true;
  const key = event.key.toLowerCase();
  return key === chord.key || key === NAMED_KEYS[chord.key];
}

/** The chord as a person reads it, for the palette and menus. */
export function describeChord(chord: Chord): string {
  const parts: string[] = [];
  if (isMac()) {
    if (chord.ctrl) parts.push("⌃");
    if (chord.alt) parts.push("⌥");
    if (chord.shift) parts.push("⇧");
    if (chord.meta) parts.push("⌘");
  } else {
    if (chord.ctrl) parts.push("Ctrl");
    if (chord.alt) parts.push("Alt");
    if (chord.shift) parts.push("Shift");
    if (chord.meta) parts.push("Meta");
  }
  const symbols: Record<string, string> = {
    up: "↑",
    down: "↓",
    left: "←",
    right: "→",
    enter: "⏎",
    tab: "⇥",
    escape: "Esc",
  };
  parts.push(symbols[chord.key] ?? chord.key.toUpperCase());
  return isMac() ? parts.join("") : parts.join("+");
}
