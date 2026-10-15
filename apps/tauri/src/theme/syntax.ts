// Syntax colour by scope, derived from a palette and emitted by the theme
// pipeline as `--syntax-*`. It imports no palette value so `tokens.ts` can
// call it without a cycle.
//
// The terminal editor does not read these: it emits the ANSI 16, which the
// canvas resolves through `--forge-ansi-*` from the same theme. They serve DOM
// surfaces that colour syntax by scope (diffs, project-search excerpts, the
// theme preview) through a `data-syntax` attribute, because a cell grid has
// sixteen slots and a stylesheet does not.

import { hex, parseHex, readableColor } from "./mix";
import type { Palette } from "./tokens";

// Syntax uses a 4:1 product floor, below WCAG AA's 4.5:1 for normal-sized text.
export const SCOPE_CONTRAST_FLOOR = 4;

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
export function syntaxScopes(active: Palette): EditorScopes {
  const ground = parseHex(active.editor);
  const text = parseHex(active.text);
  const ansi = active.ansi.map(parseHex);
  const lift = (color: number) => hex(liftToFloor(color, ground, text));
  return {
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
  };
}

/** `camelCase` to the kebab-case a custom property is spelled in. */
export function cssName(key: string): string {
  return key.replace(/([a-z0-9])([A-Z])/g, "$1-$2").toLowerCase();
}

/** `--syntax-<scope>` for every scope of this palette. */
export function syntaxVariables(active: Palette): Record<string, string> {
  return Object.fromEntries(
    Object.entries(syntaxScopes(active)).map(([scope, color]) => [
      `--syntax-${cssName(scope)}`,
      color,
    ]),
  );
}

/** One `[data-syntax]` rule per scope, so a token span carries a name instead of a colour. */
export function syntaxRules(scopes: EditorScopes): string {
  return Object.keys(scopes)
    .map((scope) => `[data-syntax="${scope}"] {\n  color: var(--syntax-${cssName(scope)});\n}\n`)
    .join("");
}
