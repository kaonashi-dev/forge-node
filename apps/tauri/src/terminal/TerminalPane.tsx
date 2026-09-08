import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { TERMINAL } from "../actions/actions";
import { enterContext, registerAction } from "../actions/dispatch";
import {
  copySelection,
  repaintTerminal,
  resizeTerminal,
  scrollTerminal,
  sendKey,
  sendMouse,
  sendPaste,
  sendText,
} from "../runtime/api";
import { cellsChannel, clipboardChannel } from "../runtime/bus";
import { setTerminalStore, terminalStore } from "../store/terminalStore";
import { metrics as tokens } from "../theme/tokens";
import { TERMINAL_ZOOM_KEY, readScale, writeChoice } from "../shell/layout";
import { LatencyProbe, PaintProbe, PAINT_BUDGET_MS } from "./latency";
import { gridSize, measureCell, type CellMetrics } from "./metrics";
import { readPalette } from "./palette";
import { TerminalRenderer } from "./renderer";
import { cellAtPoint, ends, isEmpty, wordAt, type CellPoint, type Selection } from "./selection";
import type { CellsPayload } from "./types";
import { Viewport } from "./viewport";
import { clipboardPaste } from "./clipboard";

/** Breathing room between the grid and the pane edges (`TERMINAL_PAD`). */
const PAD = 8;
/**
 * How long a resize settles before the PTY is told.
 *
 * A drag fires a `ResizeObserver` callback per frame, and each one costs the
 * daemon a `resize` plus a full resync of the grid.
 */
const RESIZE_DEBOUNCE_MS = 80;

/** Turn on the p50/p95/p99 overlay with `localStorage.forgeTerminalDebug = "1"`. */
function debugEnabled(): boolean {
  try {
    return window.localStorage.getItem("forgeTerminalDebug") === "1";
  } catch {
    // A WebView with site data blocked throws on the accessor itself.
    return false;
  }
}

/**
 * The live terminal.
 *
 * Passive by construction (ADR-011): it paints `CellGrid` rows the daemon's one
 * VT engine produced and never parses a byte of terminal output itself. Input
 * goes the other way through a hidden textarea, which is what gives the pane an
 * input method, a `paste` event and a caret the platform can place — none of
 * which a canvas has on its own.
 */
/**
 * Zoom bounds and step (§4.1 U3).
 *
 * A tenth per press: enough that one press is visible, small enough that
 * finding a comfortable size takes presses rather than luck. Half to double,
 * because below half the box-drawing glyphs stop resolving and above double a
 * standard window is under forty columns, which most TUIs will not lay out.
 */
const ZOOM_STEP = 0.1;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 2;

function readZoom(): number {
  return readScale(TERMINAL_ZOOM_KEY, ZOOM_MIN, ZOOM_MAX, 1);
}

function writeZoom(value: number): void {
  writeChoice(TERMINAL_ZOOM_KEY, String(value));
}

/** The mono size the grid is measured at, rounded to a whole pixel. */
function scaledSize(zoom: number): number {
  return Math.max(6, Math.round(tokens.monoSize * zoom));
}

export function TerminalPane() {
  let host!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  let keys!: HTMLTextAreaElement;

  const viewport = new Viewport();
  const probe = new LatencyProbe();
  const paintProbe = new PaintProbe();
  const [overlay, setOverlay] = createSignal<string | null>(null);

  let renderer: TerminalRenderer | null = null;
  /**
   * The window's terminal zoom (§4.1 U3), as a multiplier on the theme's own
   * mono size.
   *
   * A window-level preference and not a theme one: the theme's size is what
   * every *other* mono surface uses, and a person zooming a terminal to read
   * a stack trace is not asking for larger tabs. Bounded so no step can make
   * the grid unusable, and persisted through `app_state` like every other
   * `ui.*` preference.
   */
  const [zoom, setZoom] = createSignal(readZoom());
  let cell: CellMetrics = measureCell(scaledSize(zoom()), tokens.mono, tokens.monoLineHeight);
  let selection: Selection | null = null;
  let selecting = false;
  let focused = false;

  const dirty = new Set<number>();
  let repaintAll = true;
  let frame = 0;
  let settleTo = 0;
  let resizeTimer: number | undefined;
  let wheelRemainder = 0;
  /**
   * The button currently held for a program reading the mouse, or `null`.
   *
   * Held across the drag rather than re-read per event because a `mousemove`
   * reports which buttons are down as a bitmask, and a release that lands
   * outside the window would leave that mask disagreeing with what the program
   * was told was pressed.
   */
  let reporting: string | null = null;
  let lastReported = { col: -1, row: -1 };
  let lastSize = { cols: 0, rows: 0 };
  let leaveContext: (() => void) | undefined;
  const showOverlay = debugEnabled();

  // --- painting -------------------------------------------------------------

  function schedule(): void {
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      paint();
    });
  }

  function paint(): void {
    if (!renderer) return;
    renderer.selection = selection;
    renderer.focused = focused;
    placeCaret();
    if (repaintAll) {
      // Timed here and not around the whole callback: `placeCaret` touches the
      // DOM and the settle below reads the clock, and neither is the canvas.
      const startedAt = performance.now();
      renderer.paintAll(viewport);
      paintProbe.record(performance.now() - startedAt, viewport.cols * viewport.rows.length);
      repaintAll = false;
      dirty.clear();
    } else if (dirty.size > 0) {
      renderer.paintRows(viewport, dirty);
      dirty.clear();
    }
    // The sample closes *after* the pixels are down, which is the whole point
    // of measuring key-to-render rather than key-to-event.
    if (settleTo > 0) {
      probe.settle(settleTo);
      settleTo = 0;
      const p95 = probe.p95();
      if (p95 !== terminalStore.latencyP95) setTerminalStore("latencyP95", p95);
      if (showOverlay) {
        const stats = probe.percentiles();
        const paint = paintProbe.stats();
        const roundTrip = stats
          ? `p50 ${stats.p50.toFixed(1)} · p95 ${stats.p95.toFixed(1)} · p99 ${stats.p99.toFixed(1)} ms · n=${stats.count}`
          : null;
        // The second line is the §6.6 gate, and it is the half that says
        // whether an over-budget round trip is ours or the wire's.
        const painting = paint
          ? `paint p50 ${paint.p50.toFixed(2)} · p95 ${paint.p95.toFixed(2)} · max ${paint.worst.toFixed(2)} ms` +
            ` · ${paint.cells} cells${paint.p95 > PAINT_BUDGET_MS ? " · OVER" : ""}`
          : null;
        setOverlay([roundTrip, painting].filter(Boolean).join("\n") || null);
      }
    }
  }

  /**
   * Park the hidden textarea on the terminal cursor.
   *
   * An input method puts its candidate window beside the caret it can see, and
   * the only caret the platform knows about is this element's.
   */
  function placeCaret(): void {
    const x = PAD + viewport.cursor.col * cell.width;
    const y = PAD + viewport.cursor.line * cell.height;
    keys.style.left = `${Math.round(x)}px`;
    keys.style.top = `${Math.round(y)}px`;
  }

  function onFrame(payload: CellsPayload): void {
    const rows = viewport.apply(payload);
    if (payload.full) {
      repaintAll = true;
    } else {
      for (const row of rows) dirty.add(row);
    }
    if (payload.echo_id > settleTo) settleTo = payload.echo_id;
    if (payload.title !== terminalStore.title) setTerminalStore("title", payload.title);
    if (payload.scroll_offset !== terminalStore.scrollOffset) {
      setTerminalStore("scrollOffset", payload.scroll_offset);
      // The selection is anchored to grid lines, not viewport rows, so it
      // survives the scroll — but every row it touches has to repaint.
      repaintAll = true;
    }
    if (payload.scrollback_len !== terminalStore.scrollbackLen) {
      setTerminalStore("scrollbackLen", payload.scrollback_len);
    }
    schedule();
  }

  // --- geometry -------------------------------------------------------------

  function measurePane(): void {
    if (!renderer) return;
    const rect = host.getBoundingClientRect();
    const width = Math.max(0, rect.width - PAD * 2);
    const height = Math.max(0, rect.height - PAD * 2);
    if (renderer.resize(width, height)) repaintAll = true;

    const size = gridSize(width, height, cell);
    if (size.cols !== lastSize.cols || size.rows !== lastSize.rows) {
      lastSize = size;
      setTerminalStore({ cols: size.cols, rows: size.rows });
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        void resizeTerminal(size.cols, size.rows, Math.round(width), Math.round(height)).catch(
          () => undefined,
        );
      }, RESIZE_DEBOUNCE_MS);
    }
    schedule();
  }

  // --- input ----------------------------------------------------------------

  function onKeyDown(event: KeyboardEvent): void {
    // Mid-composition the browser owns the keystroke; `keyCode === 229` is the
    // pre-`isComposing` spelling of the same thing and some WebViews still
    // only send that.
    if (event.isComposing || event.keyCode === 229) return;

    // Bound chords never reach here: the keymap runs in the capture phase and
    // stops propagation on a match (`actions/dispatch.ts`). This is the guard
    // for the ones it does not claim — ⌘ plus anything unbound belongs to the
    // platform, not to the PTY.
    if (event.metaKey) return;

    if (event.shiftKey && (event.key === "PageUp" || event.key === "PageDown")) {
      event.preventDefault();
      const page = Math.max(1, viewport.rows.length - 1);
      void scrollTerminal(event.key === "PageUp" ? page : -page).catch(() => undefined);
      return;
    }

    event.preventDefault();
    void sendKey(
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
      void sendPaste(paste.text, probe.send()).catch(() => undefined);
    } else if (paste.kind === "agent") {
      // The agent reads the system clipboard itself for image attachments.
      void sendKey({ key: "v", ctrl: true, alt: false, shift: false }, probe.send()).catch(
        () => undefined,
      );
    }
  }

  function onCompositionEnd(event: CompositionEvent): void {
    const text = event.data;
    keys.value = "";
    // Committed text is typing, not a paste: it must not pick up the bracketed
    // markers a `Paste` would wrap it in.
    if (text) void sendText(text, probe.send()).catch(() => undefined);
  }

  function onInput(): void {
    // Every ordinary key was already consumed in `keydown`; anything that lands
    // in the textarea is composition scratch, which `compositionend` reports.
    keys.value = "";
  }

  /**
   * Whether this gesture belongs to the program rather than to the pane.
   *
   * The `shift` escape is the xterm convention every terminal emulator has:
   * holding it takes the mouse back for selection while a program is reading
   * it, which is the only way to copy out of one that does.
   */
  function reportsMouse(event: MouseEvent | WheelEvent): boolean {
    return viewport.modes.mouse_mode !== "Off" && !event.shiftKey;
  }

  /** Cell coordinates for the PTY: 0-based, and unaffected by the scrollback. */
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
    void sendMouse({
      button,
      kind,
      col,
      row,
      ctrl: event.ctrlKey,
      alt: event.altKey,
      shift: event.shiftKey,
    }).catch(() => undefined);
  }

  /** `MouseEvent.button` → the name the encoder knows, or `null` for the rest. */
  function buttonName(index: number): string | null {
    if (index === 0) return "left";
    if (index === 1) return "middle";
    if (index === 2) return "right";
    return null;
  }

  function onWheel(event: WheelEvent): void {
    // A program reading the mouse expects the wheel, so it goes there as a
    // report rather than scrolling our replica out from under it.
    if (reportsMouse(event)) {
      event.preventDefault();
      const perLine = event.deltaMode === 1 ? 1 : cell.height;
      wheelRemainder += -event.deltaY / perLine;
      const lines = Math.trunc(wheelRemainder);
      if (lines === 0) return;
      wheelRemainder -= lines;
      // One report per line, the way a real wheel sends them: a program that
      // moves its cursor per notch would otherwise move it once for a flick.
      const button = lines > 0 ? "wheel_up" : "wheel_down";
      for (let index = 0; index < Math.abs(lines); index += 1) {
        report(event, button, "press");
      }
      return;
    }
    // A full-screen TUI scrolls itself: stealing the wheel would scroll our
    // replica while the program under it stayed put.
    if (viewport.modes.alt_screen) return;
    event.preventDefault();
    const perLine = event.deltaMode === 1 ? 1 : cell.height;
    wheelRemainder += -event.deltaY / perLine;
    const lines = Math.trunc(wheelRemainder);
    if (lines === 0) return;
    wheelRemainder -= lines;
    void scrollTerminal(lines).catch(() => undefined);
  }

  // --- selection ------------------------------------------------------------

  function pointAt(event: MouseEvent): CellPoint {
    const rect = canvas.getBoundingClientRect();
    return cellAtPoint(
      event.clientX - rect.left,
      event.clientY - rect.top,
      cell.width,
      cell.height,
      viewport.scrollOffset,
      viewport.cols,
    );
  }

  /** Mark every row either selection touches, so the wash is put down and lifted. */
  function markSelection(...selections: (Selection | null)[]): void {
    for (const current of selections) {
      if (!current) continue;
      const [start, end] = ends(current);
      for (let line = start.line; line <= end.line; line += 1) {
        const row = line + viewport.scrollOffset;
        if (row >= 0 && row < viewport.rows.length) dirty.add(row);
      }
    }
    schedule();
  }

  function onMouseDown(event: MouseEvent): void {
    // Reporting comes first, and takes every button: a program that asked for
    // the mouse wants the right-click too, and selection is what `shift` is
    // for while it is running.
    if (reportsMouse(event)) {
      const button = buttonName(event.button);
      if (button === null) return;
      event.preventDefault();
      keys.focus({ preventScroll: true });
      reporting = button;
      report(event, button, "press");
      return;
    }
    if (event.button !== 0) return;
    // A mousedown's default action moves focus to the element under the
    // pointer. That is the canvas, which cannot take focus, so the browser
    // lands on `<body>` and undoes the `focus()` below — the pane paints but
    // never types. The pane places its own caret, so the default has to go.
    event.preventDefault();
    keys.focus({ preventScroll: true });
    const point = pointAt(event);
    const row = point.line + viewport.scrollOffset;
    const previous = selection;
    const span = (from: number, to: number): Selection => ({
      anchor: { line: point.line, col: from },
      head: { line: point.line, col: to },
    });

    // The platform's own click counting gives the two shortcuts every terminal
    // has — a word, then a line — for the cost of a comparison. Neither leaves
    // `selecting` set: the range is already chosen, and dragging on from it
    // would fight the selection it just made.
    if (event.detail >= 3) {
      selection = span(0, Math.max(viewport.cols - 1, 0));
      selecting = false;
    } else if (event.detail === 2) {
      const [from, to] = wordAt(viewport.columns(row), point.col);
      selection = span(from, to);
      selecting = false;
    } else {
      selection = { anchor: point, head: point };
      selecting = true;
    }
    markSelection(previous, selection);
  }

  function onMouseMove(event: MouseEvent): void {
    if (reporting !== null) {
      // Only on a cell boundary: a pointer crossing one cell fires dozens of
      // moves, and each one is a write to the PTY. The encoder drops motion in
      // 1000-mode, so a program that only asked for clicks is unaffected.
      const point = reportPoint(event);
      if (point.col === lastReported.col && point.row === lastReported.row) return;
      lastReported = point;
      report(event, reporting, "motion");
      return;
    }
    if (!selecting || !selection) return;
    const point = pointAt(event);
    // A pointer crossing one cell fires dozens of moves, and each repaint is a
    // row of the grid.
    if (point.line === selection.head.line && point.col === selection.head.col) return;
    const previous = selection;
    selection = { anchor: selection.anchor, head: point };
    markSelection(previous, selection);
  }

  function onMouseUp(event: MouseEvent): void {
    if (reporting !== null) {
      report(event, reporting, "release");
      reporting = null;
      return;
    }
    if (!selecting) return;
    selecting = false;
    // A plain click clears whatever was selected: dismissing a selection by
    // clicking is the gesture every terminal has.
    if (selection && isEmpty(selection)) {
      const previous = selection;
      selection = null;
      markSelection(previous);
    }
  }

  // --- lifecycle ------------------------------------------------------------

  function applyZoom(next: number): void {
    const clamped = Math.min(Math.max(Number(next.toFixed(2)), ZOOM_MIN), ZOOM_MAX);
    if (clamped === zoom()) return;
    setZoom(clamped);
    writeZoom(clamped);
    remeasure();
  }

  /** Re-measure the cell box and repaint everything against it. */
  function remeasure(): void {
    cell = measureCell(scaledSize(zoom()), tokens.mono, tokens.monoLineHeight);
    if (renderer) {
      renderer.metrics = cell;
      renderer.palette = readPalette();
      renderer.invalidateFonts();
    }
    repaintAll = true;
    measurePane();
  }

  onMount(() => {
    renderer = new TerminalRenderer(canvas, cell, readPalette());

    const stopCells = cellsChannel.subscribe(onFrame);
    const stopClipboard = clipboardChannel.subscribe((text) => {
      void writeClipboard(text);
    });

    const observer = new ResizeObserver(() => measurePane());
    observer.observe(host);
    measurePane();

    // The bundled JetBrains Mono may not be resident on the first frame, and a
    // cell box measured from the fallback shears every box-drawing TUI.
    void document.fonts?.ready.then(remeasure).catch(() => undefined);

    // The host attached before this canvas existed, so its first frame had
    // nowhere to go. Ask for it rather than wait for output.
    void repaintTerminal().catch(() => undefined);

    // The terminal is what the window opens on, so it starts focused; without
    // this the first thing typed goes nowhere until the pane is clicked.
    keys.focus({ preventScroll: true });

    // Only this pane knows what is selected and what the modes are, so the
    // clipboard and scroll actions are answered here rather than in the shell.
    const bound = [
      registerAction("copy_terminal", () => {
        if (selection && !isEmpty(selection)) {
          void copySelection(selection.anchor, selection.head).catch(() => undefined);
        }
      }),
      registerAction("paste_terminal", () => {
        void readClipboard().then((text) => {
          if (text) void sendPaste(text, probe.send()).catch(() => undefined);
        });
      }),
      // Zoom re-measures the cell, which re-derives the grid and resizes the
      // PTY: a larger glyph is fewer columns, and a program drawing a box has
      // to be told so.
      registerAction("terminal_zoom_in", () => applyZoom(zoom() + ZOOM_STEP)),
      registerAction("terminal_zoom_out", () => applyZoom(zoom() - ZOOM_STEP)),
      registerAction("terminal_zoom_reset", () => applyZoom(1)),
    ];
    onCleanup(() => {
      for (const unbind of bound) unbind();
    });

    // A drag that leaves the pane still belongs to the pane.
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);

    // A theme change moves every color, and a base switch can move the font.
    const themes = new MutationObserver(remeasure);
    themes.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "style"],
    });

    onCleanup(() => {
      stopCells();
      stopClipboard();
      observer.disconnect();
      themes.disconnect();
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
      window.clearTimeout(resizeTimer);
      if (frame !== 0) cancelAnimationFrame(frame);
      leaveContext?.();
    });
  });

  return (
    <section
      class="terminal-pane"
      aria-label="Terminal"
      ref={host}
      onMouseDown={onMouseDown}
      onWheel={onWheel}
      // A right-click that a program asked for must not also raise the
      // WebView's own menu over the grid it was aimed at.
      onContextMenu={(event) => {
        if (reportsMouse(event)) event.preventDefault();
      }}
    >
      <canvas class="terminal-canvas" ref={canvas} />
      <textarea
        class="terminal-keys"
        ref={keys}
        aria-label="Terminal input"
        autocomplete="off"
        autocapitalize="off"
        spellcheck={false}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onCopy={(event) => {
          if (!selection || isEmpty(selection)) return;
          event.preventDefault();
          void copySelection(selection.anchor, selection.head).catch(() => undefined);
        }}
        onInput={onInput}
        onCompositionEnd={onCompositionEnd}
        onFocus={() => {
          focused = true;
          // The grid holds the keyboard, so its own chords outbid the shell's.
          leaveContext = enterContext(TERMINAL);
          dirty.add(viewport.cursor.line);
          schedule();
        }}
        onBlur={() => {
          focused = false;
          leaveContext?.();
          leaveContext = undefined;
          dirty.add(viewport.cursor.line);
          schedule();
        }}
      />
      <Show when={terminalStore.scrollOffset > 0}>
        <div class="terminal-scrolled">
          {terminalStore.scrollOffset} lines back · type to return
        </div>
      </Show>
      <Show when={overlay()}>{(text) => <div class="terminal-debug">{text()}</div>}</Show>
    </section>
  );
}

/**
 * Put text on the system clipboard.
 *
 * `navigator.clipboard` is the path a Tauri WebView takes; the `execCommand`
 * fallback is for the ones that still refuse it outside a user gesture, which a
 * key handler technically is not by the time an await has resolved.
 */
/**
 * Read the clipboard for a paste the user asked for with a chord.
 *
 * The `paste` DOM event is the normal path and needs none of this; this is the
 * one the keymap takes, where no event carries the data.
 */
async function readClipboard(): Promise<string> {
  try {
    return await navigator.clipboard.readText();
  } catch {
    // A WebView that refuses the read outside a paste event: the `paste`
    // handler still works, so this is a fallback losing nothing.
    return "";
  }
}

async function writeClipboard(text: string): Promise<void> {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // Fall through to the legacy path.
  }
  const scratch = document.createElement("textarea");
  scratch.value = text;
  scratch.setAttribute("aria-hidden", "true");
  scratch.style.position = "fixed";
  scratch.style.opacity = "0";
  document.body.append(scratch);
  scratch.select();
  try {
    document.execCommand("copy");
  } finally {
    scratch.remove();
  }
}
