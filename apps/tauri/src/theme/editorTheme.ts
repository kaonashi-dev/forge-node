// The editor's own colours, derived from the same palette as everything else.
//
// Scope assignment stays importable by a node test that has no DOM
// (`AGENTS.md`, TypeScript section). The scopes themselves live in
// `syntax.ts`, which the theme pipeline also emits as `--syntax-*`.

import { hex, mix, parseHex, pct } from "./mix";
import { liftToFloor, syntaxScopes, type EditorScopes } from "./syntax";
import { palettes, type Palette, type ThemeBaseId } from "./tokens";

export { SCOPE_CONTRAST_FLOOR, liftToFloor, type EditorScopes } from "./syntax";

/** Every colour the editor paints, by role. */
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
  /** The three git gutter marks. */
  gitAdded: string;
  gitModified: string;
  gitDeleted: string;
};

export function editorPalette(base: ThemeBaseId): EditorPalette {
  const active: Palette = palettes[base];
  const ground = parseHex(active.editor);
  const text = parseHex(active.text);

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
    scopes: syntaxScopes(active),
  };
}
