import { Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { EDITOR } from "../../../actions/actions";
import { enterContext } from "../../../actions/dispatch";
import { editorChrome } from "../editorChrome";
import { EditorBreadcrumbs, EditorConflictNote, EditorStatusBar } from "../EditorChromeBars";
import { editorAnnouncement, editorAria } from "../editorAria";
import {
  repaintEditor,
  resizeEditor,
  sendEditorKey,
  sendEditorMouse,
  sendEditorPaste,
  sendEditorText,
  pasteEditorClipboard,
} from "./commands";
import { clearEditorConflict, editorConflictFor } from "../conflict/editorConflictStore";
import { CompareView } from "../conflict/CompareView";
import { Button, ContextMenu, type MenuItem } from "../../../ui/index";
import { SourceSwitch } from "../../files/preview/SourceSwitch";
import { openDiff, revealInTree } from "../../../navigation/viewsStore";
import { showView } from "../../../navigation/sidebarStore";
import { editorCellsChannel } from "../../../runtime/bus";
import { connectionStore } from "../../../state/connection";
import {
  EDITOR_FONT_SIZE_KEY,
  EDITOR_FONT_SIZE_RANGE,
  EDITOR_LINE_HEIGHT_KEY,
  EDITOR_LINE_HEIGHT_RANGE,
  readScale,
} from "../../../state/preferences";
import { forgeStore } from "../../../state/forgeStore";
import { metrics as tokens } from "../../../theme/tokens";
import { measureCell, gridSize, type CellMetrics } from "../../../shared/cell-grid/metrics";
import { readPalette } from "../../../shared/cell-grid/palette";
import { TerminalRenderer } from "../../../shared/cell-grid/renderer";
import { Viewport } from "../../../shared/cell-grid/viewport";
import { clipboardPaste } from "../../../shared/input/clipboard";
import { CursorBlink, prefersReducedMotion } from "../../../shared/cell-grid/cursorBlink";
import { editorKeyForMeta } from "./editorChords";
import type { CellsPayload } from "../../../contracts/terminal";
import { LatencyProbe } from "../../../shared/cell-grid/latency";
import {
  loadEditorConflict,
  overwriteEditorBuffer,
  reloadEditorBuffer,
} from "../conflict/commands";
import { FindPanel } from "../FindPanel";

const PAD = 8;
const RESIZE_DEBOUNCE_MS = 80;

type EditorTerminalPaneProps = {
  session: string;
  path: string;
};

/**
 * A Code-region pane for a daemon-supervised `forge-editor`.
 *
 * Same renderer, palette and textarea input path as `TerminalPane`; its own
 * frame channel so the hidden main pane is undisturbed. Chrome reads
 * `Session.editor`, never ANSI.
 */
export function EditorTerminalPane(props: EditorTerminalPaneProps) {
  let host!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  let keys!: HTMLTextAreaElement;

  const viewport = new Viewport();
  const probe = new LatencyProbe();
  let renderer: TerminalRenderer | null = null;
  let cell: CellMetrics = measureCell(tokens.monoSize, tokens.mono, tokens.monoLineHeight);
  const dirty = new Set<number>();
  let repaintAll = true;
  let frame = 0;
  let resizeTimer: number | undefined;
  let lastSize = { cols: 0, rows: 0 };
  /** Whether a left drag reported to the editor is in progress. */
  let reporting = false;
  /** The last cell a motion was reported for, to skip sub-cell moves. */
  let lastReported = { col: -1, row: -1 };
  /** Wheel lines accumulated below one whole notch. */
  let wheelRemainder = 0;
  /** Whether the hidden textarea holds the keyboard; the caret shows there. */
  let focused = false;
  const blink = new CursorBlink((visible) => {
    if (!renderer) return;
    renderer.cursorVisible = visible;
    // Only the caret cell: clearing the whole row on every phase is the wash
    // that still read as flicker once scroll frames were coalesced.
    renderer.paintCaret(viewport);
  });

  const session = createMemo(() => forgeStore.sessions.find((item) => item.id === props.session));
  const chrome = createMemo(() => editorChrome(session()?.editor, props.path));
  /* The canvas is unreadable to an accessibility tree, so the editor's own
     state is mirrored into a hidden node beside it. Never the input path: the
     textarea still takes every key. */
  const aria = createMemo(() => editorAria(session()?.editor, props.path));
  const announcement = createMemo(() => editorAnnouncement(session()?.editor));
  /** The daemon's word that a save was refused, not something read off ANSI. */
  const conflict = createMemo(() => session()?.editor?.conflict === true);
  const [comparing, setComparing] = createSignal(false);
  const [menuAt, setMenuAt] = createSignal<{ x: number; y: number } | null>(null);
  const sides = createMemo(() => editorConflictFor(props.session));

  /* A conflict that resolves takes its comparison with it: leaving the panel
     up would show two sides of a disagreement that no longer exists. */
  createEffect(() => {
    if (!conflict()) {
      setComparing(false);
      clearEditorConflict(props.session);
    }
  });

  /*
   * The pane's own menu. Copy and paste go through the *terminal's* selection
   * and the system clipboard, not the editor's internal register: what the
   * person sees highlighted is what a copy should take, and the register is
   * the editor's own scratch space.
   */
  function menuItems(): MenuItem[] {
    return [
      {
        kind: "item",
        label: "Paste",
        run: () => {
          void pasteEditorClipboard(
            props.session,
            () => navigator.clipboard.readText(),
            () => probe.send(),
          ).catch(() => undefined);
        },
      },
      { kind: "rule" },
      {
        kind: "item",
        label: "Reveal in Files",
        run: () => {
          revealInTree(chrome().path);
          showView("Files");
        },
      },
      {
        kind: "item",
        label: "Open Diff",
        run: () => openDiff(),
      },
    ];
  }

  function toggleCompare(): void {
    if (comparing()) {
      setComparing(false);
      return;
    }
    setComparing(true);
    void loadEditorConflict(props.session).catch(() => undefined);
  }
  const failed = createMemo(() => {
    const state = session()?.state;
    if (state && typeof state === "object" && "Failed" in state) {
      return state.Failed?.reason ?? "failed";
    }
    return null;
  });

  function schedule(): void {
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (!renderer) return;
      renderer.focused = focused;
      if (repaintAll) {
        renderer.paintAll(viewport);
        repaintAll = false;
        dirty.clear();
        return;
      }
      if (dirty.size === 0) return;
      renderer.paintRows(viewport, [...dirty]);
      dirty.clear();
    });
  }

  function onFrame(payload: CellsPayload): void {
    // Every open editor publishes on this one channel, so a frame is this
    // pane's only when it carries the shown session's terminal. Keyed off the
    // session's own `terminal_id` rather than the first frame seen: sharing one
    // pane across files means the session under it changes, and a sticky first
    // terminal would pin the canvas to whatever opened first.
    if (payload.terminal !== session()?.terminal_id) return;
    const rows = viewport.apply(payload);
    if (payload.full) repaintAll = true;
    else for (const row of rows) dirty.add(row);
    schedule();
  }

  createEffect(() => {
    const id = props.session;
    if (connectionStore.connection.kind !== "connected") return;
    window.clearTimeout(resizeTimer);
    lastSize = { cols: 0, rows: 0 };
    repaintAll = true;
    dirty.clear();
    void repaintEditor(id).catch(() => undefined);
    measurePane();
  });

  createEffect(() => {
    const size = readScale(
      EDITOR_FONT_SIZE_KEY,
      EDITOR_FONT_SIZE_RANGE.min,
      EDITOR_FONT_SIZE_RANGE.max,
      EDITOR_FONT_SIZE_RANGE.fallback,
    );
    const spacing = readScale(
      EDITOR_LINE_HEIGHT_KEY,
      EDITOR_LINE_HEIGHT_RANGE.min,
      EDITOR_LINE_HEIGHT_RANGE.max,
      tokens.monoLineHeight,
    );
    cell = measureCell(size, tokens.mono, spacing);
    if (renderer) {
      renderer.metrics = cell;
      renderer.invalidateFonts();
    }
    repaintAll = true;
    measurePane();
  });

  function measurePane(): void {
    if (!renderer || !host) return;
    const rect = host.getBoundingClientRect();
    const width = Math.max(0, rect.width - PAD * 2);
    const height = Math.max(0, rect.height - PAD * 2);
    if (renderer.resize(width, height)) repaintAll = true;
    const size = gridSize(width, height, cell);
    if (size.cols !== lastSize.cols || size.rows !== lastSize.rows) {
      lastSize = size;
      window.clearTimeout(resizeTimer);
      const id = props.session;
      resizeTimer = window.setTimeout(() => {
        void resizeEditor(id, size.cols, size.rows, Math.round(width), Math.round(height)).catch(
          () => undefined,
        );
      }, RESIZE_DEBOUNCE_MS);
    }
    schedule();
  }

  onMount(() => {
    renderer = new TerminalRenderer(canvas, cell, readPalette());
    // The wheel is reported to the TUI, which moves its own viewport: the grid
    // always shows the live screen, so the caret must not be hidden for a
    // scrollback offset that never changes here.
    renderer.followsScrollback = false;
    measurePane();
    const unsubscribe = editorCellsChannel.subscribe(onFrame);
    const observer = new ResizeObserver(() => measurePane());
    observer.observe(host);
    // A drag that leaves the pane still belongs to it.
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    onCleanup(enterContext(EDITOR));
    keys.focus({ preventScroll: true });
    onCleanup(() => {
      unsubscribe();
      observer.disconnect();
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
      blink.dispose();
      if (frame !== 0) cancelAnimationFrame(frame);
      window.clearTimeout(resizeTimer);
    });
  });

  function onKeyDown(event: KeyboardEvent): void {
    if (event.isComposing || event.keyCode === 229) return;
    // Typing restarts the phase *shown*, so a burst of keys never spends half
    // its frames with the caret hidden under the character about to be placed.
    blink.wake();
    if (event.metaKey) {
      const chord = editorKeyForMeta(event);
      // ⌘V and every chord the editor does not own keep their default: the
      // platform's `paste` event is what carries the clipboard into the pane.
      if (!chord) return;
      event.preventDefault();
      void sendEditorKey(props.session, chord, probe.send()).catch(() => undefined);
      return;
    }
    event.preventDefault();
    void sendEditorKey(
      props.session,
      {
        key: event.key,
        ctrl: event.ctrlKey,
        alt: event.altKey,
        shift: event.shiftKey,
      },
      probe.send(),
    ).catch(() => undefined);
  }

  function onPaste(event: ClipboardEvent): void {
    event.preventDefault();
    const paste = clipboardPaste(event.clipboardData);
    if (paste.kind === "text") {
      void sendEditorPaste(props.session, paste.text, probe.send()).catch(() => undefined);
    }
  }

  function onCompositionEnd(event: CompositionEvent): void {
    const text = event.data;
    keys.value = "";
    if (text) void sendEditorText(props.session, text, probe.send()).catch(() => undefined);
  }

  // --- mouse ----------------------------------------------------------------
  //
  // The editor reads the mouse itself (caret, selection, scroll): the pane
  // only forwards the events as reports, the way `TerminalPane` does for a TUI.
  // Unlike the terminal there is no local selection to hold back, so `shift`
  // goes through — the editor reads shift-click as an extend.

  /** Whether the editor's grid is in a mouse-reporting mode. */
  function reportsMouse(): boolean {
    return viewport.modes.mouse_mode !== "Off";
  }

  /** Cell coordinates for the editor PTY: 0-based, clamped to the grid. */
  function reportPoint(event: MouseEvent | WheelEvent): { col: number; row: number } {
    const rect = canvas.getBoundingClientRect();
    return {
      col: Math.max(
        0,
        Math.min(viewport.cols - 1, Math.floor((event.clientX - rect.left) / cell.width)),
      ),
      row: Math.max(
        0,
        Math.min(viewport.rows.length - 1, Math.floor((event.clientY - rect.top) / cell.height)),
      ),
    };
  }

  function report(event: MouseEvent | WheelEvent, button: string, kind: string): void {
    const { col, row } = reportPoint(event);
    void sendEditorMouse(props.session, {
      button,
      kind,
      col,
      row,
      ctrl: event.ctrlKey,
      alt: event.altKey,
      shift: event.shiftKey,
    }).catch(() => undefined);
  }

  function onMouseDown(event: MouseEvent): void {
    // Left button only: right-click keeps the pane's own HTML menu, and the
    // rest are out of scope for the editor.
    if (event.button !== 0 || !reportsMouse()) return;
    event.preventDefault();
    keys.focus({ preventScroll: true });
    reporting = true;
    lastReported = { col: -1, row: -1 };
    report(event, "left", "press");
  }

  function onMouseMove(event: MouseEvent): void {
    if (!reporting) return;
    // On a cell boundary only: a pointer crossing one cell fires dozens of
    // moves, and each is a write to the PTY.
    const point = reportPoint(event);
    if (point.col === lastReported.col && point.row === lastReported.row) return;
    lastReported = point;
    report(event, "left", "motion");
  }

  function onMouseUp(event: MouseEvent): void {
    if (!reporting) return;
    reporting = false;
    report(event, "left", "release");
  }

  function onWheel(event: WheelEvent): void {
    if (!reportsMouse()) return;
    event.preventDefault();
    const perLine = event.deltaMode === 1 ? 1 : cell.height;
    wheelRemainder += -event.deltaY / perLine;
    const lines = Math.trunc(wheelRemainder);
    if (lines === 0) return;
    wheelRemainder -= lines;
    // One report per line, flushed together on the next frame so the editor's
    // drain-then-paint loop sees the burst as one queue rather than N paints.
    const button = lines > 0 ? "wheel_up" : "wheel_down";
    const count = Math.abs(lines);
    for (let index = 0; index < count; index += 1) {
      report(event, button, "press");
    }
  }

  return (
    <div class="editor-terminal-pane">
      <EditorBreadcrumbs path={chrome().path}>
        <Show when={failed()}>{(reason) => <span class="panel-note">{reason()}</span>}</Show>
        <SourceSwitch path={props.path} surface="editor" />
      </EditorBreadcrumbs>

      {/* The same three answers the DOM editor offers, for the same reason:
          the draft and the other write both still exist, so throwing one away
          is a choice rather than the only way forward. */}
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
      <div
        ref={host}
        class="editor-terminal-body"
        onMouseDown={onMouseDown}
        onWheel={onWheel}
        onContextMenu={(event) => {
          event.preventDefault();
          setMenuAt({ x: event.clientX, y: event.clientY });
        }}
      >
        <Show when={session()?.editor?.find}>
          {(find) => (
            <FindPanel
              session={props.session}
              find={find}
              onReturn={() => keys.focus({ preventScroll: true })}
            />
          )}
        </Show>
        <canvas ref={canvas} />
        {/* The screen-reader mirror. `application` rather than `textbox`: the
            keys go to the textarea below, and a reader that took this for an
            input would offer its own editing keys against a node that has
            none. Everything in it comes from `Session.editor`, never from the
            cells. */}
        <div
          class="editor-terminal-aria"
          role="application"
          aria-roledescription="code editor"
          aria-label={aria().label}
          aria-readonly={aria().readOnly}
        >
          <p>{aria().status}</p>
          <p>{aria().line}</p>
        </div>
        {/* Polite, not assertive: a find tally should wait for the word being
            read rather than cut it off. */}
        <div class="editor-terminal-aria" role="status" aria-live="polite" aria-atomic="true">
          {announcement()}
        </div>
        {/* The editor owns its viewport; this only reports where it is. The
            wheel still goes to the TUI, so the thumb is not a handle. */}
        <Show when={chrome().scroll}>
          {(scroll) => (
            <div class="editor-terminal-scrollbar" aria-hidden="true">
              <div
                class="editor-terminal-thumb"
                style={{
                  top: `${scroll().top * 100}%`,
                  height: `${Math.max(scroll().size * 100, 4)}%`,
                }}
              />
            </div>
          )}
        </Show>
        <Show when={menuAt()}>
          {(at) => (
            <ContextMenu
              x={at().x}
              y={at().y}
              items={menuItems()}
              onDismiss={() => setMenuAt(null)}
            />
          )}
        </Show>
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
            focused = true;
            blink.run(!prefersReducedMotion());
            // The cursor is painted over the row, so the row it sits on is
            // dirty even though no cell changed.
            dirty.add(viewport.cursor.line);
            schedule();
          }}
          onBlur={() => {
            focused = false;
            blink.run(false);
            dirty.add(viewport.cursor.line);
            schedule();
          }}
        />
      </div>
      <EditorStatusBar chrome={chrome()} />
    </div>
  );
}
