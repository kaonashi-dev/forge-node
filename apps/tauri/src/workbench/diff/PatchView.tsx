import { createEffect, onCleanup, onMount } from "solid-js";
import { EditorState, RangeSetBuilder, type Extension } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  gutter,
  keymap,
  type DecorationSet,
} from "@codemirror/view";
import type { PatchRow } from "../patch";
import type { ThemeBaseId } from "../../theme/tokens";
import { editorTheme } from "../editor/theme";
import { intraLine, pairedRows, patchDocument, rowClass } from "./patchDocument";

/**
 * One file's patch, rendered by CodeMirror (D1–D2).
 *
 * The old renderer built a `<div>` per patch line for every expanded file, so
 * a 2 000-line patch was 2 000 rows of four spans each, all live at once. This
 * one hands the patch to a read-only `EditorView`, which draws the viewport and
 * nothing else — the same reason the editor moved (§2.1).
 *
 * Read-only and unfocusable-by-default is deliberate: this is a report, and a
 * caret in it would suggest the patch could be edited. It still takes focus on
 * click so `]c` / `[c` and the browser's own find work inside it.
 */
export function PatchView(props: {
  patch: string;
  base: ThemeBaseId;
  /** Open this file in the editor at a line of the new file (D4). */
  onOpenLine: (line: number) => void;
}) {
  let host!: HTMLDivElement;
  let view: EditorView | undefined;

  function build(): void {
    view?.destroy();
    const doc = patchDocument(props.patch);
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: doc.text,
        extensions: [
          EditorView.editable.of(false),
          EditorState.readOnly.of(true),
          EditorView.lineWrapping,
          editorTheme(props.base),
          rowDecorations(doc.rows),
          numberGutter(doc.rows, "before"),
          numberGutter(doc.rows, "after"),
          markerGutter(doc.rows),
          hunkKeymap(doc.hunkStarts),
          openLine(doc.rows, props.onOpenLine),
          patchTheme(),
        ],
      }),
    });
  }

  onMount(build);
  // Rebuilt rather than reconfigured: the decorations, both gutters and the
  // hunk index are all derived from the patch text, so a new patch is a new
  // state rather than an update to this one.
  createEffect(() => {
    props.patch;
    props.base;
    if (view) build();
  });
  onCleanup(() => view?.destroy());

  return <div ref={host} class="diff-patch" />;
}

/* ------------------------------------------------------------- decorations --- */

/** One line class per row, plus the intra-line marks for paired edits (D2). */
function rowDecorations(rows: PatchRow[]): Extension {
  const build = (view: EditorView): DecorationSet => {
    const builder = new RangeSetBuilder<Decoration>();
    const doc = view.state.doc;
    // Line decorations must be added in document order, and the intra-line
    // marks for a row have to follow that row's own line decoration — hence
    // one pass with the pairs looked up rather than a second pass over them.
    const marks = new Map<number, { from: number; to: number }>();
    for (const pair of pairedRows(rows)) {
      const spans = intraLine(rows[pair.removed].text, rows[pair.added].text);
      if (!spans) continue;
      if (spans.before.to > spans.before.from) marks.set(pair.removed, spans.before);
      if (spans.after.to > spans.after.from) marks.set(pair.added, spans.after);
    }

    for (let index = 0; index < rows.length && index < doc.lines; index += 1) {
      const line = doc.line(index + 1);
      builder.add(line.from, line.from, Decoration.line({ class: rowClass(rows[index].kind) }));
      const span = marks.get(index);
      if (!span) continue;
      const from = line.from + Math.min(span.from, line.length);
      const to = line.from + Math.min(span.to, line.length);
      if (to > from) builder.add(from, to, Decoration.mark({ class: "forge-diff-intra" }));
    }
    return builder.finish();
  };

  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = build(view);
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}

/* ----------------------------------------------------------------- gutters --- */

class NumberMarker extends GutterMarker {
  constructor(readonly value: string) {
    super();
  }
  toDOM(): Text {
    return document.createTextNode(this.value);
  }
}

/**
 * The two line-number columns a unified patch needs.
 *
 * Both, and not one: a patch's whole use is saying which line of which side a
 * change is on, and collapsing to a single column throws half of that away.
 */
function numberGutter(rows: PatchRow[], side: "before" | "after"): Extension {
  return gutter({
    class: `forge-diff-gutter forge-diff-gutter-${side}`,
    lineMarker: (view, block) => {
      const index = view.state.doc.lineAt(block.from).number - 1;
      const value = rows[index]?.[side];
      return value === null || value === undefined ? null : new NumberMarker(String(value));
    },
    // Nothing here depends on the selection or the viewport, only on the
    // document, which never changes for the life of this view.
    lineMarkerChange: () => false,
  });
}

const MARKERS: Record<string, string> = { added: "+", removed: "−" };

function markerGutter(rows: PatchRow[]): Extension {
  return gutter({
    class: "forge-diff-gutter forge-diff-gutter-marker",
    lineMarker: (view, block) => {
      const index = view.state.doc.lineAt(block.from).number - 1;
      const glyph = MARKERS[rows[index]?.kind ?? ""];
      return glyph ? new NumberMarker(glyph) : null;
    },
    lineMarkerChange: () => false,
  });
}

/* -------------------------------------------------------------- keyboard --- */

/** `]c` / `[c` — vim's chords, because a diff is where vim users arrive. */
function hunkKeymap(hunkStarts: number[]): Extension {
  const jump = (forward: boolean) => (view: EditorView) => {
    if (hunkStarts.length === 0) return false;
    const current = view.state.doc.lineAt(view.state.selection.main.head).number - 1;
    const target = forward
      ? hunkStarts.find((index) => index > current)
      : [...hunkStarts].reverse().find((index) => index < current);
    if (target === undefined) return false;
    const line = view.state.doc.line(target + 1);
    view.dispatch({
      selection: { anchor: line.from },
      effects: EditorView.scrollIntoView(line.from, { y: "start" }),
    });
    return true;
  };
  return keymap.of([
    { key: "]c", run: jump(true) },
    { key: "[c", run: jump(false) },
  ]);
}

/** D4: double-click a row to open that line of the file in the editor. */
function openLine(rows: PatchRow[], onOpenLine: (line: number) => void): Extension {
  return EditorView.domEventHandlers({
    dblclick: (event, view) => {
      const position = view.posAtCoords({ x: event.clientX, y: event.clientY });
      if (position === null) return false;
      const index = view.state.doc.lineAt(position).number - 1;
      // The *new* file's number: that is the file the editor would open. A
      // removed line has none, and there is nothing to open it at.
      const line = rows[index]?.after;
      if (line === null || line === undefined) return false;
      onOpenLine(line);
      return true;
    },
  });
}

/* ----------------------------------------------------------------- theme --- */

/**
 * The patch's own colours, on top of the editor theme.
 *
 * The two washes are the same `--forge-diff-*-bg` the old renderer used, so a
 * patch looks the same as it did; the intra-line mark is a stronger version of
 * whichever wash its line already carries.
 */
function patchTheme(): Extension {
  return EditorView.theme({
    "&": { height: "auto" },
    ".cm-scroller": { overflow: "visible" },
    ".cm-line": { padding: "0 var(--space-8)" },
    ".forge-diff-added": { backgroundColor: "var(--forge-diff-added-bg)" },
    ".forge-diff-removed": { backgroundColor: "var(--forge-diff-removed-bg)" },
    ".forge-diff-hunk": { color: "var(--fg-subtle)", backgroundColor: "var(--bg-subtle)" },
    ".forge-diff-meta": { color: "var(--fg-subtle)" },
    ".forge-diff-intra": {
      backgroundColor: "var(--bg-selected)",
      borderRadius: "var(--radius-2xs)",
    },
    ".forge-diff-gutter": {
      color: "var(--fg-subtle)",
      minWidth: "var(--space-24)",
      padding: "0 var(--space-4)",
      textAlign: "right",
      userSelect: "none",
    },
    ".forge-diff-gutter-marker": { minWidth: "var(--space-14)", textAlign: "center" },
  });
}
