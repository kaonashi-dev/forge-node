// The editor's own colours, derived from the same palette as everything else.
//
// Scope assignment stays importable by a node test that has no DOM
// (`AGENTS.md`, TypeScript section).
//
// The terminal editor does not read these: it emits the ANSI 16, which the
// canvas resolves through `--forge-ansi-*` from the same theme. These are the
// DOM surfaces that still paint text — the diff, the review and the preview
// panes — and the reason both tables exist is that a cell grid has sixteen
// slots and a stylesheet does not.

import { hex, mix, parseHex, pct, readableColor } from "./mix";
import { palettes, type Palette, type ThemeBaseId } from "./tokens";

// Syntax uses a 4:1 product floor, below WCAG AA's 4.5:1 for normal-sized text.
export const SCOPE_CONTRAST_FLOOR = 4;

/** Every colour the editor paints, by role rather than by CM6 class name. */
export type EditorPalette = {
  background: string;
  foreground: string;
  caret: string;
  selection: string;
  selectionMatch: string;
  activeLine: string;
  gutterBackground: string;
  gutterForeground: string;
  gutterActiveForeground: string;
  border: string;
  matchingBracket: string;
  searchMatch: string;
  searchMatchSelected: string;
  /** Syntax, by tag family, for the DOM surfaces that paint text. */
  scopes: EditorScopes;
  /** The three git gutter marks (A5). */
  gitAdded: string;
  gitModified: string;
  gitDeleted: string;
};

export type EditorScopes = {
  comment: string;
  keyword: string;
  controlKeyword: string;
  operator: string;
  punctuation: string;
  variable: string;
  property: string;
  function: string;
  type: string;
  namespace: string;
  string: string;
  escape: string;
  regexp: string;
  number: string;
  constant: string;
  tag: string;
  attribute: string;
  heading: string;
  link: string;
  invalid: string;
  meta: string;
};

export function liftToFloor(
  color: number,
  ground: number,
  toward: number,
  floor: number = SCOPE_CONTRAST_FLOOR,
): number {
  return readableColor(color, [ground], toward, floor);
}

/**
 * Assign every scope, then hold each one to the floor.
 *
 * The assignment leans on the ANSI brights instead of inventing a second set
 * of syntax colours: they are already the palette's answer to "eight hues that
 * read on this background", the terminal beside the editor is painted with
 * them, and a `TODO` in a comment should not be a different yellow in the two
 * panes.
 */
export function editorPalette(base: ThemeBaseId): EditorPalette {
  const active: Palette = palettes[base];
  const ground = parseHex(active.editor);
  const text = parseHex(active.text);
  const ansi = active.ansi.map(parseHex);
  const lift = (color: number) => hex(liftToFloor(color, ground, text));

  return {
    background: active.editor,
    foreground: active.text,
    caret: active.text,
    // Selection is a wash on the ground rather than a fill, so the text inside
    // it keeps the colour the grammar gave it instead of turning into one flat
    // block the moment three lines are selected.
    selection: hex(mix(ground, parseHex(active.accent), pct(26))),
    selectionMatch: hex(mix(ground, parseHex(active.accent), pct(14))),
    activeLine: hex(mix(ground, text, pct(5))),
    gutterBackground: active.editor,
    gutterForeground: hex(liftToFloor(parseHex(active.faint), ground, text, 3)),
    gutterActiveForeground: active.text,
    border: hex(mix(ground, text, pct(12))),
    matchingBracket: hex(mix(ground, parseHex(active.accent), pct(30))),
    searchMatch: hex(mix(ground, parseHex(active.amber), pct(28))),
    searchMatchSelected: hex(mix(ground, parseHex(active.needsYou), pct(40))),
    gitAdded: active.gitAdded,
    gitModified: active.gitModified,
    gitDeleted: active.gitDeleted,
    scopes: {
      comment: hex(liftToFloor(parseHex(active.faint), ground, text)),
      keyword: lift(ansi[9]),
      controlKeyword: lift(ansi[13]),
      operator: hex(liftToFloor(parseHex(active.muted), ground, text)),
      punctuation: hex(liftToFloor(parseHex(active.muted), ground, text)),
      variable: active.text,
      property: lift(ansi[12]),
      function: lift(ansi[11]),
      type: lift(ansi[14]),
      namespace: lift(ansi[14]),
      string: lift(ansi[10]),
      escape: lift(ansi[13]),
      regexp: lift(ansi[14]),
      number: lift(ansi[13]),
      constant: lift(ansi[13]),
      tag: lift(ansi[9]),
      attribute: lift(ansi[11]),
      heading: lift(ansi[11]),
      link: lift(ansi[12]),
      // The one scope allowed to shout: a lexer error is not decoration.
      invalid: active.red,
      meta: hex(liftToFloor(parseHex(active.muted), ground, text)),
    },
  };
}
