// A1: the editor, as a handle over a CodeMirror 6 view.
//
// Framework-free on purpose. `EditorView.tsx` is a Solid component and this is
// not: CM6 owns its own DOM and its own update cycle, and the one thing that
// must never happen is Solid and CM6 both deciding what is on screen. The
// component creates one of these, feeds it, and gets out of the way.
//
// ADR-012 still holds. Nothing here reads or writes a file: text arrives from
// a `ReadFile` and leaves through a `WriteFile` conditioned on the revision
// that read carried. CM6 has no filesystem access and is given none.

import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from "@codemirror/autocomplete";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  standardKeymap,
} from "@codemirror/commands";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
  type LanguageSupport,
} from "@codemirror/language";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import {
  Compartment,
  EditorState,
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
  type Extension,
} from "@codemirror/state";
import {
  EditorView,
  GutterMarker,
  crosshairCursor,
  drawSelection,
  dropCursor,
  gutterLineClass,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import { isMac } from "../../actions/keys";
import type { ThemeBaseId } from "../../theme/tokens";
import { findBar } from "./searchPanel";
import { symbolAt } from "./symbol";
import { editorTheme } from "./theme";

/** What the git gutter can say about a line (A5). */
export type GitMark = "added" | "modified" | "deleted";

/** Line number (1-based, as git counts) to the mark that line carries. */
export type GitMarks = ReadonlyMap<number, GitMark>;

export type EditorOptions = {
  doc: string;
  base: ThemeBaseId;
  /** Fired for edits the person made, never for a document set from outside. */
  onChange: (text: string) => void;
  /** A click on a git gutter mark, with the 1-based line it sat on. */
  onRevealDiff?: (line: number) => void;
  /**
   * The platform's modifier plus a click on a name, with the line it sat on.
   *
   * The chord half of the same gesture is not here: `cmd-b` is an action in
   * the `Editor` context like every other shortcut in this shell, so the
   * component asks the handle for [`EditorHandle.symbolAtCursor`] instead.
   */
  onOpenDefinition?: (symbol: string, line: number) => void;
  /** The view lost focus. Autosave's stronger signal than a pause (A8). */
  onBlur?: () => void;
  readOnly?: boolean;
};

export type EditorHandle = {
  readonly view: EditorView;
  /** The current text. Cheap enough to call on save, not on every keystroke. */
  text: () => string;
  /** Replace the document without reporting a change. */
  setDoc: (text: string) => void;
  setLanguage: (support: LanguageSupport | null) => void;
  setBase: (base: ThemeBaseId) => void;
  setGitMarks: (marks: GitMarks) => void;
  /** Put the caret on a 1-based line and scroll it into view. */
  revealLine: (line: number) => void;
  /** The name under the caret and the 1-based line it is on, or `null`. */
  symbolAtCursor: () => { symbol: string; line: number } | null;
  focus: () => void;
  destroy: () => void;
};

/* ------------------------------------------------------------ git gutter --- */

const applyGitMarks = StateEffect.define<GitMarks>();

const gitMarkField = StateField.define<GitMarks>({
  create: () => new Map(),
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(applyGitMarks)) return effect.value;
    }
    return value;
  },
});

class LineClass extends GutterMarker {
  constructor(readonly elementClass: string) {
    super();
  }
}

const GIT_CLASS: Record<GitMark, LineClass> = {
  added: new LineClass("forge-git-added"),
  modified: new LineClass("forge-git-modified"),
  deleted: new LineClass("forge-git-deleted"),
};

/**
 * Paint the marks the diff put on this file.
 *
 * `gutterLineClass` rather than a gutter of its own: a fourth column beside
 * the fold arrow and the line number would cost horizontal space on every file
 * to say something about a handful of lines, and a stripe down the inside edge
 * of the existing gutter is what every editor this plan measures against does.
 */
const gitGutter: Extension = [
  gitMarkField,
  gutterLineClass.compute([gitMarkField], (state) => {
    const marks = state.field(gitMarkField);
    if (marks.size === 0) return RangeSet.empty;
    const builder = new RangeSetBuilder<GutterMarker>();
    // A `RangeSetBuilder` insists on ascending positions, and a `Map` is in
    // insertion order — which for marks derived from a patch is hunk order,
    // not line order.
    const lines = [...marks.keys()].sort((a, b) => a - b);
    for (const line of lines) {
      if (line < 1 || line > state.doc.lines) continue;
      builder.add(
        state.doc.line(line).from,
        state.doc.line(line).from,
        GIT_CLASS[marks.get(line)!],
      );
    }
    return builder.finish();
  }),
];

/* ------------------------------------------------------------- the view --- */

/**
 * The chords CM6 is allowed to keep.
 *
 * `standardKeymap` and `defaultKeymap` between them already carry everything
 * A4 asks for — `Mod-/` comment, `Alt-Arrow` move line, `Mod-Shift-k` delete
 * line, `Mod-[`/`Mod-]` indent — so the list below adds only what CM6 does not
 * bind by default. Nothing here may shadow an app chord: `Mod-k`, `Mod-p`,
 * `Mod-w`, `Mod-b`, `Mod-j`, `Mod-t` and the digit chords are the shell's, and
 * the shell's capture-phase listener takes them before this keymap is reached
 * anyway (`plan-ui-ux.md` §8.1).
 */
function keys(): Extension {
  return keymap.of([
    ...closeBracketsKeymap,
    ...standardKeymap,
    ...defaultKeymap,
    ...searchKeymap,
    ...historyKeymap,
    ...foldKeymap,
    ...completionKeymap,
    // `Ctrl-g` for go-to-line on both platforms. CM6 binds `Mod-Alt-g`, which
    // on macOS is a dead chord under most keyboard layouts.
    { key: "Ctrl-g", run: searchKeymap.find((b) => b.key === "Mod-Alt-g")?.run ?? (() => false) },
    // Tab indents rather than leaving the editor. Last, so the completion and
    // fold keymaps get first refusal on it.
    indentWithTab,
  ]);
}

export function createEditor(host: HTMLElement, options: EditorOptions): EditorHandle {
  const language = new Compartment();
  const theme = new Compartment();
  const editable = new Compartment();

  /**
   * Suppresses `onChange` while the document is being replaced from outside.
   *
   * A reload from disk is a transaction like any other, and reporting it as an
   * edit would mark the file dirty the instant an agent's version landed —
   * which is precisely the state the conflict banner exists to distinguish.
   */
  let quiet = false;

  const view = new EditorView({
    parent: host,
    state: EditorState.create({
      doc: options.doc,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        foldGutter(),
        drawSelection(),
        dropCursor(),
        EditorState.allowMultipleSelections.of(true),
        indentOnInput(),
        indentUnit.of("  "),
        bracketMatching(),
        closeBrackets(),
        // Completion from the document alone — no LSP, which stays out of
        // scope (`plan-editor.md` §4). It is the word list the file already
        // contains, which is what makes a long identifier typeable.
        autocompletion({ activateOnTyping: false }),
        rectangularSelection(),
        crosshairCursor(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        findBar(),
        gitGutter,
        keys(),
        language.of([]),
        theme.of(editorTheme(options.base)),
        editable.of(EditorView.editable.of(!options.readOnly)),
        EditorView.updateListener.of((update) => {
          if (!quiet && update.docChanged) options.onChange(update.state.doc.toString());
          // `focusChanged` and not a DOM `blur` handler: CM6 moves focus
          // between its own content element and its panels — opening find
          // would otherwise read as leaving the editor.
          if (update.focusChanged && !update.view.hasFocus) options.onBlur?.();
        }),
        ...(options.onOpenDefinition
          ? [
              EditorView.domEventHandlers({
                /*
                 * `mousedown`, not `click`: CM6 has already moved the caret
                 * and started a selection drag by the time a click lands, so
                 * a modifier-click would leave the word highlighted behind
                 * the jump. Returning `true` is what stops that drag.
                 */
                mousedown: (event, target) => {
                  if (event.button !== 0) return false;
                  if (!(isMac() ? event.metaKey : event.ctrlKey)) return false;
                  const position = target.posAtCoords({ x: event.clientX, y: event.clientY });
                  if (position === null) return false;
                  const line = target.state.doc.lineAt(position);
                  const symbol = symbolAt(line.text, position - line.from);
                  if (!symbol) return false;
                  options.onOpenDefinition?.(symbol, line.number);
                  return true;
                },
              }),
            ]
          : []),
        ...(options.onRevealDiff
          ? [
              EditorView.domEventHandlers({
                click: (event, target) => {
                  const gutter = (event.target as HTMLElement | null)?.closest(
                    ".cm-gutterElement.forge-git-added, .cm-gutterElement.forge-git-modified, .cm-gutterElement.forge-git-deleted",
                  );
                  if (!gutter) return false;
                  const position = target.posAtCoords({ x: event.clientX, y: event.clientY });
                  if (position === null) return false;
                  options.onRevealDiff?.(target.state.doc.lineAt(position).number);
                  return true;
                },
              }),
            ]
          : []),
      ],
    }),
  });

  function setDoc(text: string): void {
    if (text === view.state.doc.toString()) return;
    quiet = true;
    try {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: text },
        // The caret is put back at the top rather than kept: the text under it
        // is a different file's text, and holding a stale offset is how a
        // reload lands the caret in the middle of a word.
        selection: { anchor: 0 },
        scrollIntoView: true,
      });
    } finally {
      quiet = false;
    }
  }

  return {
    view,
    text: () => view.state.doc.toString(),
    setDoc,
    setLanguage: (support) =>
      view.dispatch({ effects: language.reconfigure(support ? [support] : []) }),
    setBase: (base) => view.dispatch({ effects: theme.reconfigure(editorTheme(base)) }),
    setGitMarks: (marks) => view.dispatch({ effects: applyGitMarks.of(marks) }),
    revealLine: (line) => {
      if (line < 1 || line > view.state.doc.lines) return;
      const at = view.state.doc.line(line);
      view.dispatch({
        selection: { anchor: at.from },
        effects: EditorView.scrollIntoView(at.from, { y: "center" }),
      });
      view.focus();
    },
    symbolAtCursor: () => {
      const at = view.state.selection.main.head;
      const line = view.state.doc.lineAt(at);
      const symbol = symbolAt(line.text, at - line.from);
      return symbol ? { symbol, line: line.number } : null;
    },
    focus: () => view.focus(),
    destroy: () => view.destroy(),
  };
}
