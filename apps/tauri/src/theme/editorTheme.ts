// The editor's own colours, derived from the same palette as everything else.
//
// CodeMirror ships themes, and none of them is this app's theme: an editor
// painted in One Dark inside a gruvbox shell is the seam the whole token
// pipeline exists to remove. So the scopes are assigned here, from `palettes`,
// and `workbench/editor/theme.ts` turns them into CM6 extensions.
//
// Nothing in this module imports CodeMirror. The assignment is the part with a
// right and a wrong answer — a scope too dim to read is a defect — so it stays
// importable by a node test that has no DOM (`AGENTS.md`, TypeScript section).

import { contrast, hex, mix, parseHex, pct } from "./mix";
import { palettes, type Palette, type ThemeBaseId } from "./tokens";

/**
 * The contrast floor every scope colour has to clear against the editor
 * ground.
 *
 * 4:1 rather than WCAG's 4.5:1 for body text, and deliberately: this is
 * 13px monospace at the weight `--forge-mono` renders, which WCAG counts as
 * large-ish, and holding syntax colour to the body-text ratio flattens a
 * palette into four greys. It is the same floor `theming.md` puts on the
 * chrome's own muted text.
 */
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
  /** Syntax, by lezer tag family. Keys are read by `workbench/editor/theme.ts`. */
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

/**
 * Lift `color` off `ground` until it clears `floor`.
 *
 * Mixes toward `toward` — the theme's own foreground — in 4% steps rather than
 * lightening in HSL, so a colour that has to move stays recognisably itself
 * and lands somewhere the palette already contains. A colour that already
 * clears the floor is returned untouched, which is almost all of them: the
 * ANSI brights these are drawn from were chosen to be readable on this exact
 * ground in the terminal.
 */
export function liftToFloor(
  color: number,
  ground: number,
  toward: number,
  floor: number = SCOPE_CONTRAST_FLOOR,
): number {
  let current = color;
  // 25 steps of 4% reaches `toward` exactly, and `toward` is the theme's text
  // colour, which clears the floor by construction. So this always terminates
  // with an answer rather than giving up on a stubborn hue.
  for (let step = 0; step < 25 && contrast(current, ground) < floor; step += 1) {
    current = mix(current, toward, pct(4));
  }
  return current;
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
    caret: active.accent,
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
