// The file editor: a CodeMirror 6 view behind a plain handle.
//
// Framework-free on purpose — CM6 owns this DOM subtree and its own update
// cycle, and the one thing that must not happen is a host framework and CM6
// both deciding what is on screen. The host makes one, feeds it, stays away.
//
// No filesystem access, and none is given: text arrives as `doc` or `setDoc`
// and leaves through `onChange`, so every path and every write stays the host's.

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
import { highlightSelectionMatches, searchKeymap, search } from "@codemirror/search";
import {
  Compartment,
  EditorState,
  RangeSet,
  RangeSetBuilder,
  StateEffect,
  StateField,
  Transaction,
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
import { symbolAt } from "./symbol.js";

/**
 * Which modifier opens a definition: meta on Apple, control elsewhere.
 *
 * `navigator.platform` is deprecated and empty in some embeddings, so the
 * user-agent is the fallback and a host that already knows can say so with
 * `accelerator`. Getting this wrong is silent — the click just does nothing.
 */
function defaultAccelerator(): "meta" | "ctrl" {
  if (typeof navigator === "undefined") return "ctrl";
  return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent) ? "meta" : "ctrl";
}

/** What the gutter can say about a line. */
export type GitMark = "added" | "modified" | "deleted";

/** Line number (1-based, as git counts) to the mark that line carries. */
export type GitMarks = ReadonlyMap<number, GitMark>;

export type FileEditorOptions = {
  doc: string;
  theme?: Extension;
  /**
   * A find panel of the host's own.
   *
   * `searchKeymap` is bound either way, so a replacement has to be driven by
   * CM6's search commands rather than by state of its own.
   */
  findExtension?: Extension;
  /**
   * Modifier for open-definition; detected from the platform when absent.
   *
   * A host with its own keymap should pass it rather than let two answers to
   * the same question disagree.
   */
  accelerator?: "meta" | "ctrl";
  /** Line and column are 1-based, like the numbers in the gutter. */
  onCursor?: (position: { line: number; column: number; lines: number }) => void;

  /** Fired for edits a person made, never for a document set through `setDoc`. */
  onChange: () => void;

  /** A click on a gutter mark, with the 1-based line it sat on. */
  onRevealDiff?: (line: number) => void;

  /**
   * The accelerator plus a click on a name, with the 1-based line it sat on.
   *
   * The keyboard half of the gesture is not here: a host binds its own chord
   * and asks [`FileEditorHandle.symbolAtCursor`], so this view claims none.
   */
  onOpenDefinition?: (symbol: string, line: number) => void;

  /** The view lost focus — a firmer signal than a pause for a host that autosaves. */
  onBlur?: () => void;
  /** Fixed at construction: there is no setter for it. */
  readOnly?: boolean;
};

export type FileEditorHandle = {
  readonly view: EditorView;

  /** The current text. Cheap enough to call on save, not on every keystroke. */
  text: () => string;

  /**
   * Replace the document without reporting a change.
   *
   * `preservePosition` is for a re-read of the same file: caret and scroll are
   * carried over. Pass `false` when the text belongs to a different file — that
   * also drops the undo history, which must not cross files.
   */
  setDoc: (text: string, preservePosition?: boolean) => void;
  setLanguage: (support: LanguageSupport | null) => void;
  setTheme: (theme: Extension) => void;
  /** Replaces the whole set; an empty map clears the gutter. */
  setGitMarks: (marks: GitMarks) => void;

  /** Put the caret on a 1-based line and scroll it into view; past the end is ignored. */
  revealLine: (line: number) => void;

  /** The name under the caret and the 1-based line it is on, or `null`. */
  symbolAtCursor: () => { symbol: string; line: number } | null;
  focus: () => void;
  destroy: () => void;
};

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
  added: new LineClass("fw-git-added"),
  modified: new LineClass("fw-git-modified"),
  deleted: new LineClass("fw-git-deleted"),
};

/**
 * Paint the marks the host worked out from a diff.
 *
 * `gutterLineClass` rather than a gutter of its own: a fourth column beside the
 * fold arrow and the line number would spend horizontal space on every file to
 * say something about a handful of lines, so the marks become a stripe down the
 * inside edge of the gutter that is already there.
 */
const gitGutter: Extension = [
  gitMarkField,
  gutterLineClass.compute([gitMarkField], (state) => {
    const marks = state.field(gitMarkField);
    if (marks.size === 0) return RangeSet.empty;
    const builder = new RangeSetBuilder<GutterMarker>();
    // A `RangeSetBuilder` insists on ascending positions, and a `Map` iterates
    // in insertion order — hunk order for marks read off a patch, not line order.
    const lines = [...marks.keys()].sort((a, b) => a - b);
    for (const line of lines) {
      // Marks are computed against a revision that need not be this document;
      // `doc.line` throws past the end, so an outdated mark is dropped instead.
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

/**
 * The chords CM6 is allowed to keep.
 *
 * `standardKeymap` and `defaultKeymap` between them already carry comment
 * toggle, move line, delete line and indent, so this adds only what CM6 leaves
 * unbound. Nothing added here may shadow a chord the host shell owns.
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
    // Go-to-line on both platforms: CM6 binds it to `Mod-Alt-g`, a dead chord
    // under most macOS layouts. The lookup is by key string, so an upstream
    // rename degrades to a no-op rather than to a build error.
    { key: "Ctrl-g", run: searchKeymap.find((b) => b.key === "Mod-Alt-g")?.run ?? (() => false) },
    // Tab indents rather than leaving the editor. Last, so the completion and
    // fold keymaps get first refusal on it.
    indentWithTab,
  ]);
}

export function createFileEditor(host: HTMLElement, options: FileEditorOptions): FileEditorHandle {
  const language = new Compartment();
  const theme = new Compartment();
  const editable = new Compartment();
  const undo = new Compartment();

  /**
   * Suppresses `onChange` while `setDoc` replaces the document.
   *
   * A replacement is a transaction like any other, and reporting it as an edit
   * would mark the file dirty the instant someone else's version landed — the
   * one state a host's conflict handling exists to tell apart from an edit.
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
        undo.of(history()),
        foldGutter(),
        drawSelection(),
        dropCursor(),
        EditorState.allowMultipleSelections.of(true),
        indentOnInput(),
        indentUnit.of("  "),
        bracketMatching(),
        closeBrackets(),
        // Explicit completion only, and no source is registered here: the
        // candidates are whatever the configured `LanguageSupport` supplies,
        // so a grammar that offers none opens an empty popup.
        autocompletion({ activateOnTyping: false }),
        rectangularSelection(),
        crosshairCursor(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        options.findExtension ?? search({ top: true }),
        gitGutter,
        keys(),
        language.of([]),
        theme.of(options.theme ?? []),
        editable.of([
          // `editable` alone only stops DOM input: without `readOnly` the
          // keymap's own commands and a paste still change the document.
          EditorView.editable.of(!options.readOnly),
          EditorState.readOnly.of(options.readOnly ?? false),
        ]),
        EditorView.updateListener.of((update) => {
          if (!quiet && update.docChanged) options.onChange();
          if (update.docChanged || update.selectionSet) {
            const at = update.state.selection.main.head;
            const line = update.state.doc.lineAt(at);
            options.onCursor?.({
              line: line.number,
              column: at - line.from + 1,
              lines: update.state.doc.lines,
            });
          }
          // `focusChanged` and not a DOM `blur` handler: CM6 moves focus between
          // its content element and its panels, so opening find would otherwise
          // read as leaving the editor.
          if (update.focusChanged && !update.view.hasFocus) options.onBlur?.();
        }),
        ...(options.onOpenDefinition
          ? [
              EditorView.domEventHandlers({
                /*
                 * `mousedown`, not `click`: CM6 has already moved the caret and
                 * started a selection drag by the time a click lands, so a
                 * modifier-click would leave the word highlighted behind the
                 * jump. Returning `true` is what stops that drag.
                 */
                mousedown: (event, target) => {
                  if (event.button !== 0) return false;
                  const accelerator = options.accelerator ?? defaultAccelerator();
                  if (!(accelerator === "meta" ? event.metaKey : event.ctrlKey)) return false;
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
                  // The stripe is a class on the gutter element, so the class is
                  // the hit test: elsewhere in the gutter is a fold or a line
                  // number, not a request to see the hunk.
                  const gutter = (event.target as HTMLElement | null)?.closest(
                    ".cm-gutterElement.fw-git-added, .cm-gutterElement.fw-git-modified, .cm-gutterElement.fw-git-deleted",
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

  function setDoc(text: string, preservePosition = true): void {
    // Identical text is a no-op only while the position is being kept: a switch
    // to another file still has to reset undo, even between two files that
    // happen to read the same.
    if (preservePosition && text === view.state.doc.toString()) return;
    // Caret and anchor are remembered as line and column, not as offsets: a
    // re-read of the same file has moved the bytes around them.
    const old = view.state.doc.lineAt(view.state.selection.main.head);
    const column = view.state.selection.main.head - old.from;
    const anchorLine = view.state.doc.lineAt(view.state.selection.main.anchor);
    const anchorColumn = view.state.selection.main.anchor - anchorLine.from;
    // The scroll anchor is an offset into the outgoing text applied to the
    // incoming one — near enough for a re-read, meaningless for another file,
    // which is why that branch scrolls to the top instead.
    const scroll = view.scrollSnapshot();
    quiet = true;
    try {
      // A different file must never inherit the previous file's undo entries,
      // and dropping the field before the change is what keeps the replacement
      // itself out of the fresh history.
      if (!preservePosition) view.dispatch({ effects: undo.reconfigure([]) });
      const doc = EditorState.create({ doc: text }).doc;
      const head = doc.line(Math.min(old.number, doc.lines));
      const anchor = doc.line(Math.min(anchorLine.number, doc.lines));
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: text },
        selection: preservePosition
          ? {
              anchor: anchor.from + Math.min(anchorColumn, anchor.length),
              head: head.from + Math.min(column, head.length),
            }
          : { anchor: 0 },
        effects: preservePosition ? scroll : EditorView.scrollIntoView(0, { y: "start" }),
        // Never an undo step: undo after a re-read belongs to the person's last
        // edit, not to restoring the text the re-read replaced.
        annotations: Transaction.addToHistory.of(false),
      });
      if (!preservePosition) view.dispatch({ effects: undo.reconfigure(history()) });
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
    setTheme: (extension) => view.dispatch({ effects: theme.reconfigure(extension) }),
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
