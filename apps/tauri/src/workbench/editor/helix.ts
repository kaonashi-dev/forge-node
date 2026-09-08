// A10: a Helix keymap over CodeMirror's own selection model.
//
// Helix is selection-first: a motion *selects*, and the verb that follows acts
// on what is selected. `plan-helix-editor.md` was written against
// `the UI kit::InputState`, which held one private range and could not
// express that — hence a whole `text-model` crate in its §4.3. CodeMirror's
// `EditorSelection` is already n ranges with one primary, which is the Helix
// model exactly, so what was a crate is a keymap.
//
// Not a Helix emulator. What is here is the core of normal mode — the motions
// that select, the verbs that act on a selection, multiple cursors, and the
// `mi`/`ma` object pairs — because that is the part whose absence makes the
// editor unusable for someone who has the habits. Registers, macros, `:`
// commands and the space menu are deliberately absent rather than half-built.

import { EditorSelection, StateEffect, StateField, type Extension } from "@codemirror/state";
import { EditorView, keymap, showPanel, type Command } from "@codemirror/view";
import { cursorLineDown, cursorLineUp, redo, undo } from "@codemirror/commands";
import { findUnbalanced, wordBoundary } from "./helixMotions";

/** Helix has two modes the keymap has to distinguish. Insert is CM6's own. */
export type HelixMode = "normal" | "insert";

const setMode = StateEffect.define<HelixMode>();

export const helixMode = StateField.define<HelixMode>({
  create: () => "normal",
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(setMode)) return effect.value;
    return value;
  },
});

function enter(mode: HelixMode): Command {
  return (view) => {
    view.dispatch({ effects: setMode.of(mode) });
    return true;
  };
}

/* --------------------------------------------------------------- motions --- */

function selectWord(direction: 1 | -1): Command {
  return (view) => {
    const doc = view.state.doc.toString();
    view.dispatch({
      selection: EditorSelection.create(
        view.state.selection.ranges.map((range) => {
          const to = wordBoundary(doc, range.head, direction);
          return EditorSelection.range(range.head, to);
        }),
        view.state.selection.mainIndex,
      ),
      scrollIntoView: true,
    });
    return true;
  };
}

/** `x` — select the whole line, and extend a line at a time when repeated. */
const selectLine: Command = (view) => {
  const { state } = view;
  view.dispatch({
    selection: EditorSelection.create(
      state.selection.ranges.map((range) => {
        const first = state.doc.lineAt(range.from);
        const last = state.doc.lineAt(range.to);
        // Already covering whole lines: take the next one too.
        const covered = range.from === first.from && range.to === last.to;
        const end =
          covered && last.number < state.doc.lines ? state.doc.line(last.number + 1) : last;
        return EditorSelection.range(first.from, Math.min(end.to, state.doc.length));
      }),
      state.selection.mainIndex,
    ),
    scrollIntoView: true,
  });
  return true;
};

/* ----------------------------------------------------------------- verbs --- */

/**
 * `d` — delete the selection. With nothing selected, the character under it.
 *
 * `changeByRange` and not a list of changes: it maps every range through the
 * edits the others made, which is what keeps multiple cursors from deleting at
 * offsets that the cursor before them has already shifted.
 */
const deleteSelection: Command = (view) => {
  const { state } = view;
  view.dispatch(
    state.changeByRange((range) => ({
      changes: range.empty
        ? { from: range.from, to: Math.min(range.from + 1, state.doc.length) }
        : { from: range.from, to: range.to },
      range: EditorSelection.cursor(range.from),
    })),
  );
  return true;
};

/** `c` — delete the selection and enter insert. */
const changeSelection: Command = (view) => {
  deleteSelection(view);
  return enter("insert")(view);
};

/** `y` — copy the selection. The system clipboard, not a register. */
const yankSelection: Command = (view) => {
  const text = view.state.selection.ranges
    .map((range) => view.state.sliceDoc(range.from, range.to))
    .join("\n");
  if (text) void navigator.clipboard?.writeText(text).catch(() => undefined);
  return true;
};

/** `C` / `⌥C` — add a cursor on the line below / above. */
function addCursor(direction: 1 | -1): Command {
  return (view) => {
    const { state } = view;
    const main = state.selection.main;
    const line = state.doc.lineAt(main.head);
    const target = line.number + direction;
    if (target < 1 || target > state.doc.lines) return false;
    const to = state.doc.line(target);
    const column = main.head - line.from;
    const head = Math.min(to.from + column, to.to);
    view.dispatch({
      selection: EditorSelection.create(
        [...state.selection.ranges, EditorSelection.cursor(head)],
        state.selection.ranges.length,
      ),
    });
    return true;
  };
}

/** `,` — drop every cursor but the primary. */
const keepPrimary: Command = (view) => {
  const main = view.state.selection.main;
  view.dispatch({ selection: EditorSelection.create([main], 0) });
  return true;
};

/* --------------------------------------------------------------- objects --- */

const PAIRS: Record<string, [string, string]> = {
  "(": ["(", ")"],
  ")": ["(", ")"],
  "[": ["[", "]"],
  "]": ["[", "]"],
  "{": ["{", "}"],
  "}": ["{", "}"],
  '"': ['"', '"'],
  "'": ["'", "'"],
  "`": ["`", "`"],
};

/**
 * `mi(` / `ma"` — select inside or around a pair.
 *
 * Scanned outward from the caret rather than parsed. The parse tree would be
 * more correct for brackets and is no help at all for quotes, which are not
 * nodes in most grammars; one scan handles both and is the same code Helix's
 * own `match` uses in spirit.
 */
function selectPair(open: string, close: string, around: boolean): Command {
  return (view) => {
    const doc = view.state.doc.toString();
    const ranges = view.state.selection.ranges.map((range) => {
      const start = findUnbalanced(doc, range.head, open, close, -1);
      const end = findUnbalanced(doc, range.head, open, close, 1);
      if (start === null || end === null) return range;
      return around ? EditorSelection.range(start, end + 1) : EditorSelection.range(start + 1, end);
    });
    view.dispatch({ selection: EditorSelection.create(ranges, view.state.selection.mainIndex) });
    return true;
  };
}

/* ----------------------------------------------------------------- panel --- */

/** The mode pill, in CM6's own bottom panel. */
function modePanel(): Extension {
  return showPanel.of((view) => {
    const dom = document.createElement("div");
    dom.className = "forge-editor-mode";
    const paint = () => {
      const mode = view.state.field(helixMode, false) ?? "normal";
      dom.textContent = mode.toUpperCase();
      dom.dataset.mode = mode;
    };
    paint();
    return { dom, update: paint, bottom: true };
  });
}

/* ------------------------------------------------------------- the keymap --- */

/** Only fires in normal mode; insert mode is CodeMirror's own editor. */
function normal(command: Command): Command {
  return (view) =>
    (view.state.field(helixMode, false) ?? "normal") === "normal" ? command(view) : false;
}

const OBJECT_KEYS = Object.keys(PAIRS);

export function helixKeymap(): Extension {
  const objects = OBJECT_KEYS.flatMap((key) => {
    const [open, close] = PAIRS[key];
    return [
      { key: `m i ${key}`, run: normal(selectPair(open, close, false)) },
      { key: `m a ${key}`, run: normal(selectPair(open, close, true)) },
    ];
  });

  return [
    helixMode,
    modePanel(),
    keymap.of([
      { key: "Escape", run: enter("normal") },
      { key: "i", run: normal(enter("insert")) },
      { key: "a", run: normal(enter("insert")) },
      { key: "w", run: normal(selectWord(1)) },
      { key: "b", run: normal(selectWord(-1)) },
      { key: "e", run: normal(selectWord(1)) },
      { key: "x", run: normal(selectLine) },
      { key: "d", run: normal(deleteSelection) },
      { key: "c", run: normal(changeSelection) },
      { key: "y", run: normal(yankSelection) },
      { key: "C", run: normal(addCursor(1)) },
      { key: "Alt-c", run: normal(addCursor(-1)) },
      { key: ",", run: normal(keepPrimary) },
      { key: "j", run: normal(cursorLineDown) },
      { key: "k", run: normal(cursorLineUp) },
      { key: "u", run: normal(undo) },
      { key: "U", run: normal(redo) },
      ...objects,
    ]),
    // Normal mode must not type. Without this every unbound letter falls
    // through to CodeMirror's own input handling and inserts itself.
    EditorView.domEventHandlers({
      beforeinput: (event, view) => {
        if ((view.state.field(helixMode, false) ?? "normal") !== "normal") return false;
        if (event.inputType !== "insertText") return false;
        event.preventDefault();
        return true;
      },
    }),
  ];
}
