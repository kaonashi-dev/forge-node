// A2, the CodeMirror half: `EditorPalette` turned into the two extensions CM6
// needs — a `theme` for the chrome it draws itself, and a `HighlightStyle` for
// what the parser hands it.
//
// Split from `theme/editorTheme.ts` so the colour assignment stays testable in
// a node environment: importing `@codemirror/view` pulls in a module that
// expects a document.

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import type { Extension } from "@codemirror/state";
import { editorPalette, type EditorPalette } from "../../theme/editorTheme";
import type { ThemeBaseId } from "../../theme/tokens";

/**
 * The chrome CM6 paints: ground, caret, selection, gutter, active line.
 *
 * Written against literal colours rather than `var(--forge-*)` because CM6
 * builds a stylesheet per theme object and swaps whole objects through a
 * `Compartment` — the theme is already being rebuilt on a base change, so
 * indirecting through a custom property would add a layer that never varies.
 * Sizes are the exception and stay on tokens: they do not change with the base
 * and `--forge-mono-size` is the one place the editor and the terminal agree.
 */
function chrome(colors: EditorPalette, dark: boolean): Extension {
  return EditorView.theme(
    {
      "&": {
        backgroundColor: colors.background,
        color: colors.foreground,
        height: "100%",
        fontSize: "var(--forge-mono-size)",
      },
      ".cm-scroller": {
        fontFamily: "var(--font-mono)",
        lineHeight: "var(--forge-mono-lh)",
        overflow: "auto",
      },
      ".cm-content": { caretColor: colors.caret, padding: "var(--space-8) 0" },
      ".cm-cursor, .cm-dropCursor": { borderLeftColor: colors.caret, borderLeftWidth: "2px" },
      // CM6 needs both: `.cm-selectionBackground` while the view has focus and
      // `::selection` while it does not, and setting only one is how a
      // selection disappears the moment the palette is opened over it.
      "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
        backgroundColor: colors.selection,
      },
      ".cm-selectionMatch": { backgroundColor: colors.selectionMatch },
      ".cm-activeLine": { backgroundColor: colors.activeLine },
      ".cm-gutters": {
        backgroundColor: colors.gutterBackground,
        color: colors.gutterForeground,
        border: "none",
        borderRight: `1px solid ${colors.border}`,
      },
      ".cm-activeLineGutter": {
        backgroundColor: colors.activeLine,
        color: colors.gutterActiveForeground,
      },
      ".cm-foldPlaceholder": {
        backgroundColor: colors.selection,
        border: "none",
        color: colors.foreground,
        padding: "0 var(--space-4)",
      },
      "&.cm-focused .cm-matchingBracket, .cm-matchingBracket": {
        backgroundColor: colors.matchingBracket,
        outline: "none",
      },
      ".cm-searchMatch": { backgroundColor: colors.searchMatch },
      ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: colors.searchMatchSelected },
      // The go-to-line panel is CM6's own markup; these pull it onto the
      // shell's surfaces so it does not read as a browser widget inside the
      // app. The find bar is ours — `editor/searchPanel.ts`.
      ".cm-panels": {
        backgroundColor: "var(--bg-raised)",
        color: "var(--fg-default)",
      },
      ".cm-panels.cm-panels-top": { borderBottom: `1px solid ${colors.border}` },
      ".cm-panels.cm-panels-bottom": { borderTop: `1px solid ${colors.border}` },
      ".cm-panels input, .cm-panels button, .cm-panels select": {
        backgroundColor: "var(--bg-base)",
        border: `1px solid ${colors.border}`,
        borderRadius: "var(--radius-xs)",
        color: "var(--fg-default)",
        font: "inherit",
        padding: "var(--space-2) var(--space-4)",
      },
      ".cm-tooltip": {
        backgroundColor: "var(--bg-overlay)",
        border: `1px solid ${colors.border}`,
        borderRadius: "var(--radius-xs)",
        color: "var(--fg-default)",
      },
      // A5: three gutter marks, one class each, coloured from the git tokens.
      ".cm-gutterElement.forge-git-added": { boxShadow: `inset 2px 0 0 ${colors.gitAdded}` },
      ".cm-gutterElement.forge-git-modified": { boxShadow: `inset 2px 0 0 ${colors.gitModified}` },
      ".cm-gutterElement.forge-git-deleted": { boxShadow: `inset 2px 0 0 ${colors.gitDeleted}` },
    },
    { dark },
  );
}

function highlight(colors: EditorPalette): Extension {
  const s = colors.scopes;
  return syntaxHighlighting(
    HighlightStyle.define([
      { tag: [t.comment, t.lineComment, t.blockComment], color: s.comment, fontStyle: "italic" },
      { tag: t.docComment, color: s.comment },
      { tag: [t.keyword, t.modifier, t.self, t.definitionKeyword], color: s.keyword },
      { tag: [t.controlKeyword, t.moduleKeyword, t.operatorKeyword], color: s.controlKeyword },
      { tag: [t.operator, t.derefOperator, t.compareOperator, t.logicOperator], color: s.operator },
      {
        tag: [t.punctuation, t.separator, t.bracket, t.paren, t.brace, t.squareBracket],
        color: s.punctuation,
      },
      { tag: [t.name, t.variableName, t.definition(t.variableName)], color: s.variable },
      { tag: [t.propertyName, t.definition(t.propertyName)], color: s.property },
      {
        tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName],
        color: s.function,
      },
      { tag: [t.typeName, t.className, t.definition(t.typeName)], color: s.type },
      { tag: [t.namespace, t.labelName], color: s.namespace },
      { tag: [t.string, t.special(t.string)], color: s.string },
      { tag: t.escape, color: s.escape },
      { tag: t.regexp, color: s.regexp },
      { tag: [t.number, t.integer, t.float], color: s.number },
      {
        tag: [t.bool, t.null, t.atom, t.constant(t.variableName), t.standard(t.name)],
        color: s.constant,
      },
      { tag: [t.tagName, t.angleBracket], color: s.tag },
      { tag: [t.attributeName], color: s.attribute },
      { tag: [t.attributeValue], color: s.string },
      { tag: [t.heading, t.heading1, t.heading2, t.heading3], color: s.heading, fontWeight: "600" },
      { tag: t.strong, fontWeight: "600" },
      { tag: t.emphasis, fontStyle: "italic" },
      { tag: [t.link, t.url], color: s.link, textDecoration: "underline" },
      { tag: [t.meta, t.processingInstruction, t.annotation], color: s.meta },
      { tag: t.invalid, color: colors.scopes.invalid },
      { tag: t.strikethrough, textDecoration: "line-through" },
    ]),
  );
}

/** Both halves of a base's editor theme, ready for a `Compartment`. */
export function editorTheme(base: ThemeBaseId): Extension {
  const colors = editorPalette(base);
  // Every base this app ships is dark; the flag is what tells CM6 which of its
  // own defaults to fall back on for anything not named above.
  return [chrome(colors, true), highlight(colors)];
}
