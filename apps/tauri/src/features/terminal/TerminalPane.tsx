import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { TERMINAL } from "../../actions/actions";
import { enterContext, registerAction } from "../../actions/dispatch";
import {
  copySelection,
  moveCursor,
  pasteClipboard,
  repaintTerminal,
  resizeTerminal,
  scrollTerminal,
  sendKey,
  sendMouse,
  sendPaste,
  sendText,
} from "./commands";
import { cellsChannel, clipboardChannel } from "../../runtime/bus";
import { setTerminalStore, terminalStore } from "./terminalStore";
import { metrics as tokens } from "../../theme/tokens";
import {
  TERMINAL_ZOOM_KEY,
  TERMINAL_ZOOM_RANGE,
  TERMINAL_ZOOM_STEP,
  readScale,
  writeChoice,
} from "../../state/preferences";
import { LatencyProbe, PaintProbe, PAINT_BUDGET_MS } from "../../shared/cell-grid/latency";
import { gridSize, measureCell, type CellMetrics } from "../../shared/cell-grid/metrics";
import { readPalette } from "../../shared/cell-grid/palette";
import { TerminalRenderer, type LinkSpan } from "../../shared/cell-grid/renderer";
import {
  cellAtPoint,
  ends,
  isEmpty,
  wordAt,
  type CellPoint,
  type Selection,
} from "../../shared/cell-grid/selection";
import type { CellsPayload } from "../../contracts/terminal";
import { Viewport } from "../../shared/cell-grid/viewport";
import { CursorBlink, prefersReducedMotion } from "../../shared/cell-grid/cursorBlink";
import { indexOfCell, lineTextAt, spansOfRange } from "./links";
import { isMac } from "../../actions/keys";
import { linkedRefs, openPathRef, warmPathIndex } from "../files/references/pathLinks";
import { refAt, type PathRef } from "../files/references/pathref";
import { clipboardPaste } from "../../shared/input/clipboard";
import { mayTakeCaret, registerTerminalFocus } from "./focus";
import { connectionStore, sessionSelectionPending } from "../../state/connection";
import { centerMode } from "../../navigation/viewsStore";
import { registerFileTerminal } from "../files/explorer/fileDrag";
import { CursorClick } from "./cursorClick";
import { SelectionDrag } from "./selectionDrag";
import { readPrompt, samePrompt, type AnswerPrompt } from "./answerPrompt";
import { clearQuestion, hasQuestion, markQuestion } from "./questions";
import { forgeStore } from "../../state/forgeStore";
import { Button, IconButton } from "../../ui/index";
import { Icon } from "../../theme/icons/index";

/** Breathing room between the grid and the pane edges (`TERMINAL_PAD`). */
const PAD = 8;
/**
 * How long a resize settles before the PTY is told.
 *
 * A drag fires a `ResizeObserver` callback per frame, and each one costs the
 * daemon a `resize` plus a full resync of the grid.
 */
const RESIZE_DEBOUNCE_MS = 80;
/** How long frames settle before a waiting screen is re-read for its question. */
const PROMPT_READ_MS = 150;
/** The answer card turns this many of a menu's options into buttons. */
const CARD_CHOICES = 2;
const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "Fn"]);

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
 * Zoom bounds and step.
 *
 * A tenth per press: enough that one press is visible, small enough that
 * finding a comfortable size takes presses rather than luck. Half to double,
 * because below half the box-drawing glyphs stop resolving and above double a
 * standard window is under forty columns, which most TUIs will not lay out.
 */
function readZoom(): number {
  return readScale(
    TERMINAL_ZOOM_KEY,
    TERMINAL_ZOOM_RANGE.min,
    TERMINAL_ZOOM_RANGE.max,
    TERMINAL_ZOOM_RANGE.fallback,
  );
}

function writeZoom(value: number): void {
  writeChoice(TERMINAL_ZOOM_KEY, String(value));
}

/** The mono size the grid is measured at, rounded to a whole pixel. */
function scaledSize(zoom: number): number {
  return Math.max(6, Math.round(tokens.monoSize * zoom));
}

export function TerminalPane(props: { active?: boolean }) {
  let host!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  let keys!: HTMLTextAreaElement;

  const viewport = new Viewport();
  const probe = new LatencyProbe();
  const paintProbe = new PaintProbe();
  const [overlay, setOverlay] = createSignal<string | null>(null);

  let renderer: TerminalRenderer | null = null;
  /**
   * The window's terminal zoom, as a multiplier on the theme's own
   * mono size.
   *
   * A window-level preference and not a theme one: the theme's size is what
   * every *other* mono surface uses, and a person zooming a terminal to read
   * a stack trace is not asking for larger tabs. Bounded so no step can make
   * the grid unusable, and persisted through `app_state` like every other
   * `ui.*` preference.
   */
  const [zoom, setZoom] = createSignal(readZoom());
  /*
   * The pane is mounted before the daemon has answered, so the line above
   * reads an empty `app_state` and lands on 1 whatever the person last chose.
   * Re-read when the snapshot arrives, and re-measure: the cell box below was
   * measured against the wrong size.
   *
   * `setZoom` and not `applyZoom`: this is reading the stored value back, not
   * choosing one, and writing it again would be a round trip per launch.
   */
  createEffect(() => {
    const stored = readZoom();
    if (stored === zoom()) return;
    setZoom(stored);
    remeasure();
  });
  let cell: CellMetrics = measureCell(scaledSize(zoom()), tokens.mono, tokens.monoLineHeight);
  const cursorClick = new CursorClick();
  const selection = new SelectionDrag({
    viewport,
    geometry: () => {
      const rect = canvas.getBoundingClientRect();
      return { left: rect.left, top: rect.top, cellWidth: cell.width, cellHeight: cell.height };
    },
    changed: (previous, next) => markSelection(previous, next),
    scroll: scrollTerminal,
  });
  let focused = false;

  const dirty = new Set<number>();
  let repaintAll = true;
  let frame = 0;
  let settleTo = 0;
  let resizeTimer: number | undefined;
  let wheelRemainder = 0;
  /** Lines the wheel has asked for since the last `scroll` went out. */
  let scrollPending = 0;
  let scrollFrame = 0;
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

  /**
   * The path under the pointer while a modifier is held, and the caret's
   * blink phase — both presentation, so both live on this side of the wire.
   */
  let hovered: { ref: PathRef; spans: LinkSpan[] } | null = null;
  let hoveredCell = { row: -1, col: -1 };
  const blink = new CursorBlink((visible) => {
    if (!renderer) return;
    renderer.cursorVisible = visible;
    dirty.add(viewport.cursor.line);
    schedule();
  });

  /*
   * The answer card. Read off the rows only while the attached session has an
   * open question, and then at most once per `PROMPT_READ_MS` of frames: a
   * terminal nobody is waiting on costs one boolean per frame.
   */
  const [prompt, setPrompt] = createSignal<AnswerPrompt | null>(null);
  const [cardHidden, setCardHidden] = createSignal(false);
  let asking = false;
  let promptTimer: number | undefined;

  function readQuestion(): void {
    promptTimer = undefined;
    if (!asking || viewport.scrollOffset > 0) return;
    if (viewport.terminal === null || viewport.terminal !== connectionStore.activeTerminal) return;
    const next = readPrompt(viewport.rows);
    if (samePrompt(prompt(), next)) return;
    setPrompt(next);
    setCardHidden(false);
  }

  function scheduleQuestionRead(): void {
    if (promptTimer !== undefined) return;
    promptTimer = window.setTimeout(readQuestion, PROMPT_READ_MS);
  }

  createEffect(() => {
    asking = props.active !== false && hasQuestion(connectionStore.activeSession);
    if (asking) {
      readQuestion();
      return;
    }
    window.clearTimeout(promptTimer);
    promptTimer = undefined;
    setPrompt(null);
  });

  function noteBell(terminal: string): void {
    const session = connectionStore.activeSession;
    if (terminal !== connectionStore.activeTerminal || !session) return;
    const row = forgeStore.sessions.find((item) => item.id === session);
    if (row?.agent_provider_id != null) markQuestion(session);
  }

  function answered(): void {
    clearQuestion(connectionStore.activeSession);
  }

  function focusTerminalInput(): void {
    keys.focus({ preventScroll: true });
  }

  // The digit alone: agent menus select on the number, and a trailing Enter
  // would land on whatever the agent shows next.
  function answerWith(digit: string): void {
    void sendText(digit, probe.send()).catch(() => undefined);
    answered();
    focusTerminalInput();
  }

  createEffect(() => {
    if (props.active !== false) return;
    leaveContext?.();
    leaveContext = undefined;
    focused = false;
    keys?.blur();
    blink.run(false);
  });

  createEffect(
    on(
      [
        () => props.active,
        () => connectionStore.activeSession,
        () => connectionStore.activeTerminal,
        () => connectionStore.connectionGeneration,
        () => connectionStore.connection.kind,
        sessionSelectionPending,
      ],
      () => resetInteraction(),
    ),
  );

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
    renderer.selection = selection.range;
    renderer.focused = focused;
    renderer.link = hovered?.spans ?? [];
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
        // The repaint metric shows
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
    if (
      payload.terminal !== viewport.terminal ||
      payload.modes.alt_screen !== viewport.modes.alt_screen ||
      payload.modes.mouse_mode !== viewport.modes.mouse_mode
    ) {
      resetInteraction();
    }
    selection.syncTerminal(payload.terminal);
    const rows = viewport.apply(payload);
    selection.refresh();
    if (payload.bell) noteBell(payload.terminal);
    if (asking && (payload.full || rows.length > 0)) scheduleQuestionRead();
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

    // Typing restarts the phase *shown*: a burst of keys would otherwise spend
    // half its frames with the caret hidden under the character about to be
    // placed, which reads as dropped input.
    blink.wake();

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
    if (!MODIFIER_KEYS.has(event.key)) answered();
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
    if (paste.kind !== "empty") answered();
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
    if (!text) return;
    answered();
    void sendText(text, probe.send()).catch(() => undefined);
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

  /**
   * Send at most one `scroll` per frame, carrying everything the wheel asked
   * for since the last one.
   *
   * A trackpad fires wheel events faster than the display refreshes, and each
   * `scroll` costs a **full** frame: `cells::frame` sends every row whenever
   * `scroll_offset > 0`, because a damage list is expressed in live-viewport
   * rows and means nothing against a window of the scrollback. One command per
   * event was therefore one whole grid encoded, sent over IPC, decoded and
   * repainted *per event* — more than the pipe or the canvas could keep up
   * with, which is what made a flick look stepped rather than smooth.
   *
   * Coalescing to the frame the result would be painted on costs nothing in
   * responsiveness: the paint was already going to wait for that frame. It
   * cuts the work to one grid per frame, and opposite deltas inside a frame
   * cancel, which is what reversing mid-flick means.
   */
  function queueScroll(lines: number): void {
    scrollPending += lines;
    if (scrollFrame !== 0) return;
    scrollFrame = requestAnimationFrame(() => {
      scrollFrame = 0;
      const delta = scrollPending;
      scrollPending = 0;
      if (delta === 0) return;
      void scrollTerminal(delta).catch(() => undefined);
    });
  }

  function onWheel(event: WheelEvent): void {
    if (!acceptsPointer()) return;
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
    queueScroll(lines);
  }

  // --- selection ------------------------------------------------------------

  function acceptsPointer(): boolean {
    return (
      props.active !== false &&
      !sessionSelectionPending() &&
      viewport.terminal !== null &&
      viewport.rows.length > 0 &&
      connectionStore.connection.kind === "connected" &&
      connectionStore.activeTerminal === viewport.terminal
    );
  }

  function stopDrag(): void {
    selection.stop();
    cursorClick.cancel();
  }

  function resetInteraction(): void {
    selection.reset();
    cursorClick.cancel();
    reporting = null;
    lastReported = { col: -1, row: -1 };
    clearLink();
    if (scrollFrame !== 0) cancelAnimationFrame(scrollFrame);
    scrollFrame = 0;
    scrollPending = 0;
    wheelRemainder = 0;
  }

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
      const first = Math.max(0, start.line + viewport.scrollOffset);
      const last = Math.min(viewport.rows.length - 1, end.line + viewport.scrollOffset);
      for (let row = first; row <= last; row += 1) {
        dirty.add(row);
      }
    }
    schedule();
  }

  function onMouseDown(event: MouseEvent): void {
    if (!acceptsPointer()) return;
    stopDrag();
    // Before mouse reporting: the modifier is the user overriding whatever the
    // program asked for, the same way `shift` overrides it for selection.
    if (event.button === 0 && hovered && openModifier(event)) {
      event.preventDefault();
      openPathRef(hovered.ref);
      return;
    }
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
    cursorClick.begin(event, point, viewport);
    const span = (from: number, to: number): Selection => ({
      anchor: { line: point.line, col: from },
      head: { line: point.line, col: to },
    });

    if (event.detail >= 3) {
      selection.set(span(0, Math.max(viewport.cols - 1, 0)));
    } else if (event.detail === 2) {
      const [from, to] = wordAt(viewport.columns(row), point.col);
      selection.set(span(from, to));
    } else {
      selection.begin(event);
    }
  }

  /**
   * The path the pointer is over, while the open-modifier is held.
   *
   * Behind a modifier so an ordinary drag over output never underlines
   * anything, and so a click on a path is a deliberate gesture rather than
   * something a mis-aimed selection can trigger. Recomputed only when the cell
   * changes: a pointer crossing one cell fires dozens of moves, and each one
   * would otherwise join a wrapped line into a string and re-scan it.
   */
  function trackLink(event: MouseEvent): void {
    if (!openModifier(event)) {
      clearLink();
      return;
    }
    const point = pointAt(event);
    const row = point.line + viewport.scrollOffset;
    if (hovered && hoveredCell.row === row && hoveredCell.col === point.col) return;
    hoveredCell = { row, col: point.col };

    const line = lineTextAt(viewport.rows, row, viewport.cols);
    const index = indexOfCell(line, row, point.col);
    const ref = index < 0 ? null : refAt(linkedRefs(line.text), index);
    const previous = hovered;
    hovered = ref ? { ref, spans: spansOfRange(line, ref.from, ref.to) } : null;
    markLink(previous, hovered);
  }

  /** The platform's "follow this" chord — the same one a browser link takes. */
  function openModifier(event: MouseEvent): boolean {
    return isMac() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  }

  function clearLink(): void {
    if (!hovered) return;
    const previous = hovered;
    hovered = null;
    hoveredCell = { row: -1, col: -1 };
    markLink(previous, null);
  }

  /** Repaint only the rows the underline moved on or off. */
  function markLink(
    previous: { spans: LinkSpan[] } | null,
    next: { spans: LinkSpan[] } | null,
  ): void {
    for (const span of [...(previous?.spans ?? []), ...(next?.spans ?? [])]) {
      if (span.row >= 0 && span.row < viewport.rows.length) dirty.add(span.row);
    }
    schedule();
  }

  function onMouseMove(event: MouseEvent): void {
    if (!acceptsPointer()) return;
    if (selection.dragging && (event.buttons & 1) === 0) stopDrag();
    cursorClick.move(event);
    trackLink(event);
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
    selection.move(event);
  }

  function onMouseUp(event: MouseEvent): void {
    if (!acceptsPointer()) {
      stopDrag();
      return;
    }
    const target = cursorClick.finish(event, pointAt(event), viewport);
    if (reporting !== null) {
      report(event, reporting, "release");
      reporting = null;
      return;
    }
    if (!selection.dragging || event.button !== 0) return;
    selection.move(event);
    selection.stop();
    if (selection.range && isEmpty(selection.range)) {
      selection.set(null);
      if (target) {
        blink.wake();
        void moveCursor(target, probe.send()).catch(() => undefined);
      }
    }
  }

  // --- lifecycle ------------------------------------------------------------

  function applyZoom(next: number): void {
    const clamped = Math.min(
      Math.max(Number(next.toFixed(2)), TERMINAL_ZOOM_RANGE.min),
      TERMINAL_ZOOM_RANGE.max,
    );
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

    // Path links resolve a guessed reference against the checkout's listing;
    // without one every `App.tsx` in output would be a link to nothing.
    warmPathIndex();

    // The only thing that moves the caret across a session switch; `./focus`
    // has why the pane cannot do it on mount alone.
    const takeCaret = () => keys.focus({ preventScroll: true });
    onCleanup(
      registerFileTerminal(host, {
        identity: () => {
          const session = connectionStore.activeSession;
          const terminal = connectionStore.activeTerminal;
          return connectionStore.connection.kind === "connected" &&
            centerMode() === "session" &&
            session &&
            terminal
            ? { session, terminal }
            : null;
        },
        focus: takeCaret,
      }),
    );
    onCleanup(registerTerminalFocus(takeCaret));
    createEffect(() => {
      if (mayTakeCaret(centerMode(), connectionStore.activeSession, document.activeElement, keys)) {
        takeCaret();
      }
    });

    // Only this pane knows what is selected and what the modes are, so the
    // clipboard and scroll actions are answered here rather than in the shell.
    const bound = [
      registerAction("copy_terminal", () => {
        const range = selection.range;
        if (acceptsPointer() && range && !isEmpty(range)) {
          void copySelection(range.anchor, range.head).catch(() => undefined);
        }
      }),
      registerAction("paste_terminal", () => {
        void pasteClipboard(readClipboard).catch(() => undefined);
      }),
      // Zoom re-measures the cell, which re-derives the grid and resizes the
      // PTY: a larger glyph is fewer columns, and a program drawing a box has
      // to be told so.
      registerAction("terminal_zoom_in", () => applyZoom(zoom() + TERMINAL_ZOOM_STEP)),
      registerAction("terminal_zoom_out", () => applyZoom(zoom() - TERMINAL_ZOOM_STEP)),
      registerAction("terminal_zoom_reset", () => applyZoom(1)),
    ];
    onCleanup(() => {
      for (const unbind of bound) unbind();
    });

    // A drag that leaves the pane still belongs to the pane.
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    window.addEventListener("blur", stopDrag);

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
      window.removeEventListener("blur", stopDrag);
      stopDrag();
      window.clearTimeout(resizeTimer);
      window.clearTimeout(promptTimer);
      blink.dispose();
      if (frame !== 0) cancelAnimationFrame(frame);
      if (scrollFrame !== 0) cancelAnimationFrame(scrollFrame);
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
          const range = selection.range;
          if (!acceptsPointer() || !range || isEmpty(range)) return;
          event.preventDefault();
          void copySelection(range.anchor, range.head).catch(() => undefined);
        }}
        onInput={onInput}
        onCompositionEnd={onCompositionEnd}
        onFocus={() => {
          if (props.active === false) {
            keys.blur();
            return;
          }
          focused = true;
          // The grid holds the keyboard, so its own chords outbid the shell's.
          leaveContext = enterContext(TERMINAL);
          // A blink is an invitation to type, and an unfocused pane is not
          // taking any. Reduced motion parks it too.
          blink.run(!prefersReducedMotion());
          dirty.add(viewport.cursor.line);
          schedule();
        }}
        onBlur={() => {
          stopDrag();
          focused = false;
          leaveContext?.();
          leaveContext = undefined;
          blink.run(false);
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
      <Show when={!cardHidden() && prompt()}>
        {(current) => (
          <section
            class="answer-card"
            aria-label="Waiting on your answer"
            onMouseDown={(event) => event.stopPropagation()}
            onWheel={(event) => event.stopPropagation()}
          >
            <div class="answer-card-head">
              <span class="forge-attention-dot" aria-hidden="true" />
              <span class="answer-card-label">Waiting on your answer</span>
              <IconButton
                label="Hide the answer card"
                size="xs"
                class="answer-card-hide"
                onClick={() => setCardHidden(true)}
              >
                <Icon name="close" class="forge-icon-muted" size={12} />
              </IconButton>
            </div>
            <p class="answer-card-question">
              {current().question ?? "The agent is waiting for input in the terminal."}
            </p>
            <div class="answer-card-actions">
              <Show
                when={current().choices.length > 0}
                fallback={
                  <Button variant="primary" size="sm" onClick={() => focusTerminalInput()}>
                    Answer in the terminal
                  </Button>
                }
              >
                <For each={current().choices.slice(0, CARD_CHOICES)}>
                  {(choice, index) => (
                    <Button
                      variant={index() === 0 ? "primary" : "secondary"}
                      size="sm"
                      aria-label={`Answer ${choice.digit}: ${choice.label}`}
                      onClick={() => answerWith(choice.digit)}
                    >
                      <span class="answer-card-digit" aria-hidden="true">
                        {choice.digit}
                      </span>
                      <span class="answer-card-choice">{choice.label}</span>
                    </Button>
                  )}
                </For>
                <span class="answer-card-spacer" />
                <button
                  type="button"
                  class="answer-card-reply"
                  onClick={() => focusTerminalInput()}
                >
                  {current().choices.length > CARD_CHOICES
                    ? `${current().choices.length - CARD_CHOICES} more in the terminal · or type a reply`
                    : "or type a reply"}
                </button>
              </Show>
            </div>
          </section>
        )}
      </Show>
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
