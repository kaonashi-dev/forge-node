import { createEffect, onCleanup, onMount } from "solid-js";
import { EditorState, RangeSetBuilder, type Extension } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  gutter,
  type DecorationSet,
} from "@codemirror/view";
import { parsePatch, type PatchRow } from "../patch";
import type { ThemeBaseId } from "../../theme/tokens";
import { editorTheme } from "../editor/theme";
import { splitRows } from "./patchDocument";

/**
 * The same patch, one side per column (D2).
 *
 * Two views rather than one with two columns of text, because the two sides
 * have different line counts and CodeMirror lays out one document per view.
 * They are kept level by `splitRows`, which inserts a filler row wherever a
 * side has nothing opposite the other — without it the panes drift apart by
 * the net line count of every hunk above.
 *
 * Their scroll is tied together on `scroll`, not by a shared state field: the
 * two documents have the same number of *rows* by construction, so matching
 * `scrollTop` is exact rather than approximate.
 */
export function SplitPatchView(props: {
  patch: string;
  base: ThemeBaseId;
  onOpenLine: (line: number) => void;
}) {
  let leftHost!: HTMLDivElement;
  let rightHost!: HTMLDivElement;
  let left: EditorView | undefined;
  let right: EditorView | undefined;
  /** Guards the echo: setting one pane's scroll fires the other's handler. */
  let syncing = false;

  function pane(
    host: HTMLElement,
    side: "left" | "right",
    rows: Array<PatchRow | null>,
    other: () => EditorView | undefined,
  ): EditorView {
    return new EditorView({
      parent: host,
      state: EditorState.create({
        doc: rows.map((row) => row?.text ?? "").join("\n"),
        extensions: [
          EditorView.editable.of(false),
          EditorState.readOnly.of(true),
          editorTheme(props.base),
          sideDecorations(rows, side),
          sideGutter(rows, side),
          splitTheme(),
          EditorView.domEventHandlers({
            scroll: (_event, view) => {
              if (syncing) return false;
              const partner = other();
              if (!partner) return false;
              syncing = true;
              partner.scrollDOM.scrollTop = view.scrollDOM.scrollTop;
              syncing = false;
              return false;
            },
            dblclick: (event, view) => {
              const position = view.posAtCoords({ x: event.clientX, y: event.clientY });
              if (position === null) return false;
              const line = rows[view.state.doc.lineAt(position).number - 1]?.after;
              if (line === null || line === undefined) return false;
              props.onOpenLine(line);
              return true;
            },
          }),
        ],
      }),
    });
  }

  function build(): void {
    left?.destroy();
    right?.destroy();
    const pairs = splitRows(parsePatch(props.patch));
    left = pane(
      leftHost,
      "left",
      pairs.map((pair) => pair.left),
      () => right,
    );
    right = pane(
      rightHost,
      "right",
      pairs.map((pair) => pair.right),
      () => left,
    );
  }

  onMount(build);
  createEffect(() => {
    props.patch;
    props.base;
    if (left) build();
  });
  onCleanup(() => {
    left?.destroy();
    right?.destroy();
  });

  return (
    <div class="diff-split">
      <div ref={leftHost} class="diff-split-side" />
      <div ref={rightHost} class="diff-split-side" />
    </div>
  );
}

/**
 * A side shows only its own edits.
 *
 * The left column marks removals and the right marks additions; a filler row
 * gets neither and is drawn as an empty, tinted gap so the eye can see that
 * the other side gained a line here.
 */
function sideDecorations(rows: Array<PatchRow | null>, side: "left" | "right"): Extension {
  const own = side === "left" ? "removed" : "added";
  const build = (view: EditorView): DecorationSet => {
    const builder = new RangeSetBuilder<Decoration>();
    const doc = view.state.doc;
    for (let index = 0; index < rows.length && index < doc.lines; index += 1) {
      const row = rows[index];
      const line = doc.line(index + 1);
      const kind = row === null ? "filler" : row.kind === own ? own : row.kind;
      builder.add(line.from, line.from, Decoration.line({ class: `forge-diff-${kind}` }));
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

class NumberMarker extends GutterMarker {
  constructor(readonly value: string) {
    super();
  }
  toDOM(): Text {
    return document.createTextNode(this.value);
  }
}

function sideGutter(rows: Array<PatchRow | null>, side: "left" | "right"): Extension {
  const field = side === "left" ? "before" : "after";
  return gutter({
    class: "forge-diff-gutter",
    lineMarker: (view, block) => {
      const row = rows[view.state.doc.lineAt(block.from).number - 1];
      const value = row?.[field];
      return value === null || value === undefined ? null : new NumberMarker(String(value));
    },
    lineMarkerChange: () => false,
  });
}

function splitTheme(): Extension {
  return EditorView.theme({
    "&": { height: "auto" },
    ".cm-scroller": { overflow: "auto" },
    ".cm-line": { padding: "0 var(--space-8)", whiteSpace: "pre" },
    ".forge-diff-added": { backgroundColor: "var(--forge-diff-added-bg)" },
    ".forge-diff-removed": { backgroundColor: "var(--forge-diff-removed-bg)" },
    ".forge-diff-hunk": { color: "var(--fg-subtle)", backgroundColor: "var(--bg-subtle)" },
    ".forge-diff-meta": { color: "var(--fg-subtle)" },
    ".forge-diff-filler": { backgroundColor: "var(--bg-subtle)", opacity: "0.5" },
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
  });
}
