import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { EDITOR } from "../../../actions/actions";
import { enterContext } from "../../../actions/dispatch";
import { editorChrome } from "../editorChrome";
import { EditorBreadcrumbs, EditorConflictNote, EditorStatusBar } from "../EditorChromeBars";
import { editorAnnouncement, editorAria } from "../editorAria";
import { sendEditorSurfaceInput, setEditorSurfaceView } from "../commands";
import { clearEditorConflict, editorConflictFor } from "../conflict/editorConflictStore";
import { CompareView } from "../conflict/CompareView";
import { Button } from "../../../ui/index";
import { SourceSwitch } from "../../files/preview/SourceSwitch";
import { editorFrameChannel } from "../../../runtime/bus";
import { forgeStore } from "../../../state/forgeStore";
import {
  EDITOR_FONT_SIZE_KEY,
  EDITOR_FONT_SIZE_RANGE,
  EDITOR_LINE_HEIGHT_KEY,
  EDITOR_LINE_HEIGHT_RANGE,
  readScale,
} from "../../../state/preferences";
import { metrics as tokens } from "../../../theme/tokens";
import { measureCell } from "../../../shared/cell-grid/metrics";
import { clipboardPaste } from "../../../shared/input/clipboard";
import { inputFor, modifiersOf, textInput } from "./editorKeys";
import { editorInputBatch } from "./editorInputBatch";
import { sameWindow, scrollForCaret, windowFor, type EditorWindow } from "./editorWindow";
import type {
  EditorFrame,
  EditorMark,
  EditorPlace,
  EditorRange,
  EditorRow,
  EditorScope,
  EditorSeverity,
} from "../../../contracts/terminal";
import {
  loadEditorConflict,
  overwriteEditorBuffer,
  reloadEditorBuffer,
} from "../conflict/commands";
import { FindPanel } from "../FindPanel";

type EditorViewProps = {
  session: string;
  path: string;
};

/** A painted band, in the content layer's own pixels. */
type Rect = { x: number; y: number; width: number };

/** The glyph and colour each gutter mark reads as, as the TUI draws them. */
const MARK_GLYPH: Readonly<Record<EditorMark, string>> = {
  Added: "\u2502",
  Modified: "\u2502",
  Deleted: "\u2582",
};
const MARK_TOKEN: Readonly<Record<EditorMark, string>> = {
  Added: "var(--forge-git-added)",
  Modified: "var(--forge-git-modified)",
  Deleted: "var(--forge-git-deleted)",
};

/* A problem outranks a change on the one cell: somebody who just ran a checker
   is looking for what it found, and a gutter that grew a column the first time
   would shift every line of the file sideways. */
const SEVERITY_GLYPH: Readonly<Record<EditorSeverity, string>> = {
  Error: "\u2717",
  Warning: "\u26a0",
  Info: "\u2139",
};
const SEVERITY_TOKEN: Readonly<Record<EditorSeverity, string>> = {
  Error: "var(--forge-red)",
  Warning: "var(--forge-amber)",
  Info: "var(--forge-blue)",
};

/** What the one mark cell shows for a row, and in what colour. */
function gutterMark(row: EditorRow): { glyph: string; color: string } | null {
  if (row.diagnostic !== null) {
    return { glyph: SEVERITY_GLYPH[row.diagnostic], color: SEVERITY_TOKEN[row.diagnostic] };
  }
  if (row.mark !== null) {
    return { glyph: MARK_GLYPH[row.mark], color: MARK_TOKEN[row.mark] };
  }
  return null;
}

/**
 * The text node and offset a point falls on, across the two spellings.
 *
 * Standards say `caretPositionFromPoint`; WebKit — which is what the macOS
 * webview is — still only has `caretRangeFromPoint`. Kept out of the handler
 * because the branch is about the browser, not about the editor.
 */
function caretOffsetAt(x: number, y: number): { node: Node; offset: number } | null {
  const standard = (
    document as Document & {
      caretPositionFromPoint?: (
        x: number,
        y: number,
      ) => { offsetNode: Node; offset: number } | null;
    }
  ).caretPositionFromPoint;
  if (typeof standard === "function") {
    const position = standard.call(document, x, y);
    return position ? { node: position.offsetNode, offset: position.offset } : null;
  }
  const legacy = (
    document as Document & {
      caretRangeFromPoint?: (x: number, y: number) => Range | null;
    }
  ).caretRangeFromPoint;
  if (typeof legacy === "function") {
    const range = legacy.call(document, x, y);
    return range ? { node: range.startContainer, offset: range.startOffset } : null;
  }
  return null;
}

/** The theme colour each scope reads in, as the TUI already maps them. */
const SCOPE_TOKEN: Readonly<Record<EditorScope, string>> = {
  Plain: "var(--forge-term-fg)",
  Comment: "var(--forge-ansi-8)",
  Keyword: "var(--forge-ansi-13)",
  ControlKeyword: "var(--forge-ansi-9)",
  String: "var(--forge-ansi-10)",
  Number: "var(--forge-ansi-11)",
  Type: "var(--forge-ansi-14)",
  Function: "var(--forge-ansi-12)",
  Property: "var(--forge-ansi-6)",
  Constant: "var(--forge-ansi-3)",
};

/**
 * The Code-region surface for a headless `forge-editor`.
 *
 * One DOM node per visible line and nothing else: the container scrolls
 * natively, rows are positioned by their line number rather than by their
 * place in the array, and a frame that arrives while a person is scrolling
 * replaces the rows it names without touching the scroll position. There is no
 * canvas, no cell grid and no second copy of the document — the host owns the
 * text and this paints the window it publishes.
 */
export function EditorView(props: EditorViewProps) {
  return (
    <Show when={props.session} keyed>
      {(session) => <EditorSessionView session={session} path={props.path} />}
    </Show>
  );
}

function EditorSessionView(props: EditorViewProps) {
  let scroller!: HTMLDivElement;
  let keys!: HTMLTextAreaElement;
  let rowLayer!: HTMLDivElement;

  const cell = createMemo(() =>
    measureCell(
      readScale(
        EDITOR_FONT_SIZE_KEY,
        EDITOR_FONT_SIZE_RANGE.min,
        EDITOR_FONT_SIZE_RANGE.max,
        EDITOR_FONT_SIZE_RANGE.fallback,
      ),
      tokens.mono,
      readScale(
        EDITOR_LINE_HEIGHT_KEY,
        EDITOR_LINE_HEIGHT_RANGE.min,
        EDITOR_LINE_HEIGHT_RANGE.max,
        EDITOR_LINE_HEIGHT_RANGE.fallback,
      ),
    ),
  );
  const lineHeight = createMemo(() => Math.max(1, Math.round(cell().height)));
  const fontSize = createMemo(() => cell().fontSize);

  const [frame, setFrame] = createSignal<EditorFrame | null>(null);
  const [focused, setFocused] = createSignal(false);
  const [comparing, setComparing] = createSignal(false);
  /* Where the caret is in pixels, measured off the mounted row rather than
     computed from a column: a proportional glyph, a ligature and a wide
     character all make a column the wrong unit for an x. */
  const [caretAt, setCaretAt] = createSignal<{ x: number; y: number } | null>(null);
  /* One rect per mounted line a selection touches, measured the same way. */
  const [selectionRects, setSelectionRects] = createSignal<Rect[]>([]);
  const [extraCarets, setExtraCarets] = createSignal<{ x: number; y: number }[]>([]);
  const [decorRects, setDecorRects] = createSignal<{ kind: string; rects: Rect[] }[]>([]);

  const session = createMemo(() => forgeStore.sessions.find((item) => item.id === props.session));
  const chrome = createMemo(() => editorChrome(session()?.editor, props.path));
  const aria = createMemo(() => editorAria(session()?.editor, props.path));
  const announcement = createMemo(() => editorAnnouncement(session()?.editor));
  const conflict = createMemo(() => session()?.editor?.conflict === true);
  const sides = createMemo(() => editorConflictFor(props.session));

  const { push, flush } = editorInputBatch(props.session, sendEditorSurfaceInput);
  /** The last window asked for, so an idle scroll sends nothing. */
  let lastWindow: EditorWindow | null = null;
  /** Whether a drag reported to the host is in progress. */
  let dragging = false;
  let viewFrame = 0;
  /** The version on screen; an older frame is a reorder, not an update. */
  let shownVersion = -1;

  /** Ask the host for the window this scroll position needs. */
  function publishView(): void {
    if (!scroller) return;
    const view = windowFor(
      scroller.scrollTop,
      scroller.clientHeight,
      lineHeight(),
      frame()?.total_lines ?? 0,
    );
    if (sameWindow(lastWindow, view)) return;
    lastWindow = view;
    void setEditorSurfaceView(props.session, view.firstLine, view.lineCount).catch(() => undefined);
  }

  /* One request per animation frame, never one per scroll event: a wheel
     burst fires dozens, and the browser has already moved the content for all
     of them. */
  function onScroll(): void {
    if (viewFrame !== 0) return;
    viewFrame = requestAnimationFrame(() => {
      viewFrame = 0;
      publishView();
    });
  }

  onMount(() => {
    const unsubscribe = editorFrameChannel.subscribe((payload) => {
      if (payload.session_id !== props.session) return;
      // A coalesced burst can put two frames on the socket out of the order
      // they were built in; the older one has nothing the newer one is
      // missing.
      if (payload.frame.doc_version < shownVersion) return;
      shownVersion = payload.frame.doc_version;
      setFrame(payload.frame);
    });
    onCleanup(unsubscribe);
    onCleanup(enterContext(EDITOR));
    keys.focus({ preventScroll: true });
    publishView();
    const observer = new ResizeObserver(() => publishView());
    observer.observe(scroller);
    onCleanup(() => observer.disconnect());
    onCleanup(() => {
      if (viewFrame !== 0) cancelAnimationFrame(viewFrame);
      flush();
    });
  });

  createEffect((previous?: number) => {
    const height = lineHeight();
    if (previous !== undefined && previous !== height) {
      lastWindow = null;
      publishView();
    }
    return height;
  });

  /* A conflict that resolves takes its comparison with it. */
  createEffect(() => {
    if (!conflict()) {
      setComparing(false);
      clearEditorConflict(props.session);
    }
  });

  createEffect((previous?: EditorPlace) => {
    const current = frame();
    const height = lineHeight();
    if (current === null) return;
    const wanted = scrollForCaret(
      current.caret,
      previous,
      scroller.scrollTop,
      scroller.clientHeight,
      height,
    );
    if (wanted !== null) scroller.scrollTop = wanted;
    setCaretAt(measurePlace(current.caret));
    setSelectionRects(current.selection.flatMap(measureRange));
    setExtraCarets(current.extra_carets.map(measurePlace).filter((at) => at !== null));
    // Find hits and the bracket the caret is beside, measured the same way a
    // selection is: a decoration is a range, and a range is not a column.
    setDecorRects(
      current.decorations.map((decoration) => ({
        kind: decoration.kind,
        rects: measureRange(decoration.range),
      })),
    );
    return current.caret;
  });

  /**
   * The bands one range covers, one per mounted line it touches.
   *
   * A range can start above the window and end below it; only the lines that
   * are actually mounted can be measured, and the rest need no rect because
   * nothing is showing them.
   */
  function measureRange(range: EditorRange): Rect[] {
    const rects: Rect[] = [];
    for (const { line } of frame()?.rows ?? []) {
      if (line < range.from.line || line > range.to.line) continue;
      const row = rowLayer?.querySelector<HTMLElement>(`[data-line="${line}"]`);
      const text = row?.querySelector<HTMLElement>(".ed-text");
      if (!row || !text) continue;
      const origin = row.getBoundingClientRect().left;
      const start =
        line === range.from.line
          ? measurePlace(range.from)
          : { x: text.getBoundingClientRect().left - origin, y: 0 };
      // A selection that swallowed the line break reads to the end of the row,
      // which is what makes a multi-line selection look like one shape.
      const end =
        line === range.to.line
          ? measurePlace(range.to)
          : { x: text.getBoundingClientRect().right - origin + cell().width, y: 0 };
      if (start === null || end === null) continue;
      rects.push({ x: start.x, y: line * lineHeight(), width: Math.max(1, end.x - start.x) });
    }
    return rects;
  }

  /**
   * The UTF-16 column a point falls on, in the row it was clicked in.
   *
   * Asked of the browser rather than computed from an x: the column on this
   * wire is a UTF-16 offset, and the only thing that knows where one lands
   * after a font has had its say is the font.
   */
  function columnAt(row: HTMLElement, clientX: number, clientY: number): number {
    const text = row.querySelector<HTMLElement>(".ed-text");
    if (!text) return 0;
    const hit = caretOffsetAt(clientX, clientY);
    if (hit === null) return text.textContent?.length ?? 0;
    let column = 0;
    for (const node of text.childNodes) {
      const run = node.firstChild ?? node;
      if (run === hit.node || node === hit.node) return column + hit.offset;
      column += (node.textContent ?? "").length;
    }
    return column;
  }

  function onRowPointerDown(event: MouseEvent): void {
    // Clicking the empty space under the last row still takes the keyboard:
    // a pane that only focuses on a hit row reads as dead below the file.
    keys.focus();
    const row = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-line]");
    if (!row) return;
    const line = Number(row.dataset.line);
    if (!Number.isFinite(line)) return;
    event.preventDefault();
    dragging = true;
    push({
      Pointer: {
        kind: "Down",
        line,
        column: columnAt(row, event.clientX, event.clientY),
        modifiers: modifiersOf(event),
      },
    });
  }

  function onRowPointerMove(event: MouseEvent): void {
    if (!dragging) return;
    const row = document
      .elementFromPoint(event.clientX, event.clientY)
      ?.closest<HTMLElement>("[data-line]");
    if (!row) return;
    const line = Number(row.dataset.line);
    if (!Number.isFinite(line)) return;
    push({
      Pointer: {
        kind: "Drag",
        line,
        column: columnAt(row, event.clientX, event.clientY),
        modifiers: modifiersOf(event),
      },
    });
  }

  function onRowPointerUp(event: MouseEvent): void {
    if (!dragging) return;
    dragging = false;
    push({ Pointer: { kind: "Up", line: 0, column: 0, modifiers: modifiersOf(event) } });
  }

  /**
   * Where a place sits in the row layer, or `null` when its row is not mounted.
   *
   * Measured with a `Range` over the row's own text nodes: the column is a
   * UTF-16 offset, and only the browser knows what that is in pixels once a
   * font has had its say.
   */
  function measurePlace(place: EditorPlace): { x: number; y: number } | null {
    const row = rowLayer?.querySelector<HTMLElement>(`[data-line="${place.line}"]`);
    const text = row?.querySelector<HTMLElement>(".ed-text");
    if (!row || !text) return null;
    // Measured against the row, not the text node, so the gutter's width is
    // already in the number and the caret needs no second copy of it.
    const origin = row.getBoundingClientRect().left;
    const y = place.line * lineHeight();
    let remaining = place.column;
    for (const node of text.childNodes) {
      const run = node.textContent ?? "";
      if (remaining > run.length) {
        remaining -= run.length;
        continue;
      }
      const target = node.firstChild ?? node;
      if (target.nodeType !== Node.TEXT_NODE) break;
      const range = document.createRange();
      range.setStart(target, remaining);
      range.setEnd(target, remaining);
      return { x: range.getBoundingClientRect().left - origin, y };
    }
    // Past the last span is the end of the line, which is where a caret on an
    // empty row and a caret at the end of a row both belong.
    const rect = text.getBoundingClientRect();
    return { x: rect.right - origin, y };
  }

  function onKeyDown(event: KeyboardEvent): void {
    // The host owns every binding; the surface refuses only what the browser
    // itself would take. A bare Cmd/Ctrl chord the editor does not use still
    // travels, because deciding that here would be a second key table.
    const input = inputFor(event);
    if (input === null) return;
    event.preventDefault();
    push(input);
  }

  function onPaste(event: ClipboardEvent): void {
    event.preventDefault();
    // An image or a file on the clipboard is not text this buffer can hold;
    // `clipboardPaste` is what already knows the difference.
    const paste = clipboardPaste(event.clipboardData);
    if (paste.kind === "text") push(textInput(paste.text));
  }

  function onCompositionEnd(event: CompositionEvent): void {
    // The composed text, not the keys that built it: an IME's intermediate
    // states are not edits.
    if (event.data.length > 0) push(textInput(event.data));
    keys.value = "";
  }

  function toggleCompare(): void {
    const next = !comparing();
    setComparing(next);
    if (next) void loadEditorConflict(props.session).catch(() => undefined);
  }

  const totalHeight = createMemo(() => (frame()?.total_lines ?? 1) * lineHeight());
  const gutterWidth = createMemo(() => {
    const digits = String(Math.max(1, frame()?.total_lines ?? 1)).length;
    return Math.ceil(digits * cell().width) + 16;
  });

  return (
    <div class="editor-view">
      <EditorBreadcrumbs path={chrome().path}>
        <SourceSwitch path={props.path} surface="editor" />
      </EditorBreadcrumbs>

      {/* The host's open prompt — find, replace, go-to-line. A surface with no
          status row has to show it somewhere, or typing into find is invisible.
          A panel of real inputs is what this becomes; the keys still go to the
          host either way. */}
      <Show when={session()?.editor?.status}>
        {(status) => <div class="editor-view-prompt">{status()}</div>}
      </Show>

      <Show when={conflict()}>
        <EditorConflictNote>
          <Button
            variant="secondary"
            size="xs"
            onClick={() => void overwriteEditorBuffer(props.session).catch(() => undefined)}
          >
            Keep mine
          </Button>
          <Button
            variant="secondary"
            size="xs"
            onClick={() => void reloadEditorBuffer(props.session).catch(() => undefined)}
          >
            Take disk
          </Button>
          <Button variant="secondary" size="xs" selected={comparing()} onClick={toggleCompare}>
            Compare
          </Button>
        </EditorConflictNote>
      </Show>
      <Show when={comparing()}>
        <Show when={sides().error}>
          {(error) => <p class="panel-error">Could not read the conflict: {error()}</p>}
        </Show>
        <Show when={sides().conflict}>
          {(both) => <CompareView disk={both().disk} mine={both().mine} />}
        </Show>
      </Show>

      <Show when={session()?.editor?.find}>
        {(find) => <FindPanel session={props.session} find={find} onReturn={() => keys.focus()} />}
      </Show>

      {/* The surface *is* the accessibility tree here: a real multiline
          textbox, not a canvas with a mirror beside it. The textarea below
          still takes the keys, because a contenteditable would give the
          browser an editing model that disagrees with the host's. */}
      <div
        ref={scroller}
        class="editor-view-scroll"
        role="textbox"
        aria-multiline="true"
        aria-label={aria().label}
        aria-readonly={aria().readOnly}
        tabIndex={-1}
        onScroll={onScroll}
        onMouseDown={onRowPointerDown}
        onMouseMove={onRowPointerMove}
        onMouseUp={onRowPointerUp}
        onMouseLeave={onRowPointerUp}
        style={{
          "--ed-line-height": `${lineHeight()}px`,
          "--ed-gutter": `${gutterWidth()}px`,
          "--ed-font-size": `${fontSize()}px`,
        }}
      >
        <div class="editor-view-content" style={{ height: `${totalHeight()}px` }}>
          {/* Under the rows, never over them: a band drawn on top would grey
              the text it is meant to be highlighting. */}
          <div class="editor-view-selection" aria-hidden="true">
            <For each={selectionRects()}>
              {(rect) => (
                <div
                  class="ed-selection"
                  style={{
                    transform: `translate(${rect.x}px, ${rect.y}px)`,
                    width: `${rect.width}px`,
                    height: `${lineHeight()}px`,
                  }}
                />
              )}
            </For>
            <For each={decorRects()}>
              {(group) => (
                <For each={group.rects}>
                  {(rect) => (
                    <div
                      class="ed-decoration"
                      classList={{ [`ed-decoration-${group.kind.toLowerCase()}`]: true }}
                      style={{
                        transform: `translate(${rect.x}px, ${rect.y}px)`,
                        width: `${rect.width}px`,
                        height: `${lineHeight()}px`,
                      }}
                    />
                  )}
                </For>
              )}
            </For>
          </div>
          <div ref={rowLayer} class="editor-view-rows">
            <For each={frame()?.rows ?? []}>
              {(row) => (
                <div
                  class="ed-row"
                  classList={{ "ed-row-active": row.line === frame()?.caret.line }}
                  data-line={row.line}
                  style={{ top: `${row.line * lineHeight()}px` }}
                >
                  <span class="ed-number" aria-hidden="true">
                    {row.line + 1}
                  </span>
                  {/* One cell, always drawn: a gutter that widened the first
                      time git answered would shift every line sideways. */}
                  <span
                    class="ed-mark"
                    aria-hidden="true"
                    style={{ color: gutterMark(row)?.color ?? "transparent" }}
                  >
                    {gutterMark(row)?.glyph ?? " "}
                  </span>
                  <span
                    class="ed-fold"
                    classList={{ "ed-fold-toggle": row.fold !== null }}
                    aria-hidden="true"
                  >
                    {row.fold === null ? " " : row.fold > 0 ? "\u25b8" : "\u25be"}
                  </span>
                  <span class="ed-text">
                    <For each={row.spans}>
                      {(span) => (
                        <span style={{ color: SCOPE_TOKEN[span.scope] }}>{span.text}</span>
                      )}
                    </For>
                    <Show when={row.truncated}>
                      <span class="ed-truncated" aria-hidden="true">
                        {" …"}
                      </span>
                    </Show>
                    <Show when={(row.fold ?? 0) > 0}>
                      <span class="ed-folded" aria-hidden="true">
                        {`  … ${row.fold} lines`}
                      </span>
                    </Show>
                  </span>
                </div>
              )}
            </For>
          </div>
          {/* A terminal has one hardware cursor and paints the rest as cells;
              the DOM has no such limit, so every caret is a caret. */}
          <For each={extraCarets()}>
            {(at) => (
              <div
                class="ed-caret ed-caret-extra"
                aria-hidden="true"
                style={{
                  transform: `translate(${at.x}px, ${at.y}px)`,
                  height: `${lineHeight()}px`,
                }}
              />
            )}
          </For>
          <Show when={caretAt()}>
            {(at) => (
              <div
                class="ed-caret"
                classList={{ "ed-caret-on": focused() }}
                aria-hidden="true"
                style={{
                  transform: `translate(${at().x}px, ${at().y}px)`,
                  height: `${lineHeight()}px`,
                }}
              />
            )}
          </Show>
        </div>
        <div class="editor-terminal-aria" role="status" aria-live="polite" aria-atomic="true">
          {announcement()}
        </div>
        <textarea
          ref={keys}
          class="terminal-keys"
          autocomplete="off"
          autocapitalize="off"
          spellcheck={false}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onCompositionEnd={onCompositionEnd}
          onInput={() => {
            keys.value = "";
          }}
          onFocus={() => {
            setFocused(true);
            push({ Focus: true });
          }}
          onBlur={() => {
            setFocused(false);
            push({ Focus: false });
          }}
        />
      </div>
      <EditorStatusBar chrome={chrome()} />
    </div>
  );
}
