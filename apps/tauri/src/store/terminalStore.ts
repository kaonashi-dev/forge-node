import { createStore } from "solid-js/store";

/**
 * What the chrome needs to know about the terminal.
 *
 * Deliberately small and deliberately *not* the frame: the rows go straight to
 * the canvas (see `runtime/bus.ts`), and only the handful of values something
 * outside the pane reads — the status bar's p95, the tab strip's OSC title —
 * are written here, at most a few times a second.
 */
export const [terminalStore, setTerminalStore] = createStore({
  /** The terminal-reported (OSC 0/2) title of the attached session. */
  title: null as string | null,
  /** Key-to-render p95 over the rolling window, in milliseconds. */
  latencyP95: null as number | null,
  /** Lines the viewport is pulled up into history; 0 is the live output. */
  scrollOffset: 0,
  /** Total lines the daemon holds for this terminal. */
  scrollbackLen: 0,
  /** Grid geometry the pane last asked the PTY for. */
  cols: 0,
  rows: 0,
});
