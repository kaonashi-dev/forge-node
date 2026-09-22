import type { KeyPress } from "../../contracts/terminal";
import { sendRuntimeCommand } from "../../runtime/host";
import { connectionStore } from "../../state/connection";
import type { CursorTarget } from "./cursorClick";

export async function sendKey(key: KeyPress, id: number): Promise<void> {
  await sendRuntimeCommand({ type: "input", key, id });
}

export async function moveCursor(target: CursorTarget, id: number): Promise<void> {
  await sendRuntimeCommand({ type: "move_cursor", ...target, id });
}

/** Text an input method committed: typed, so never bracketed. */
export async function sendText(text: string, id: number): Promise<void> {
  await sendRuntimeCommand({ type: "input_text", text, id });
}

/**
 * A mouse event for a program that enabled mouse reporting.
 *
 * Only sent while the terminal is in a reporting mode and `shift` is not held:
 * the pane keeps the mouse for selection otherwise, and `shift` is the xterm
 * convention for taking it back while a program has it.
 */
export async function sendMouse(event: {
  /** `left`, `middle`, `right`, `wheel_up`, `wheel_down`. */
  button: string;
  /** `press`, `release`, `motion`. */
  kind: string;
  /** 0-based cell coordinates. */
  col: number;
  row: number;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
}): Promise<void> {
  await sendRuntimeCommand({ type: "mouse", ...event });
}

export async function sendPaste(text: string, id: number): Promise<void> {
  await sendRuntimeCommand({ type: "paste", text, id });
}

export async function sendTargetedPaste(
  session: string,
  terminal: string,
  text: string,
  generation = connectionStore.connectionGeneration,
): Promise<void> {
  await sendRuntimeCommand({
    type: "paste_target",
    session_id: session,
    terminal_id: terminal,
    text,
    connection_generation: generation,
  });
}

export async function pasteClipboard(read: () => Promise<string | null>): Promise<void> {
  const { activeSession, activeTerminal, connectionGeneration } = connectionStore;
  if (!activeSession || !activeTerminal) return;
  const text = await read();
  if (!text || connectionGeneration !== connectionStore.connectionGeneration) return;
  await sendTargetedPaste(activeSession, activeTerminal, text, connectionGeneration);
}

export async function resizeTerminal(
  cols: number,
  rows: number,
  pixelWidth: number,
  pixelHeight: number,
): Promise<void> {
  await sendRuntimeCommand({
    type: "resize",
    size: { cols, rows, pixel_width: pixelWidth, pixel_height: pixelHeight },
  });
}

/** Positive moves into history, negative back towards the live output. */
export async function scrollTerminal(lines: number): Promise<void> {
  await sendRuntimeCommand({ type: "scroll", lines });
}

export async function scrollToBottom(): Promise<void> {
  await sendRuntimeCommand({ type: "scroll_to_bottom" });
}

/**
 * Ask for the whole viewport again.
 *
 * The host connects and attaches before the WebView exists, so the frame that
 * came with the attach had no canvas to reach; the pane asks for one when it
 * mounts rather than waiting for output an idle shell will never produce.
 */
export async function repaintTerminal(): Promise<void> {
  await sendRuntimeCommand({ type: "repaint" });
}

/**
 * Ask the host to cut a dragged range.
 *
 * Only the grid knows what a run's cells hold, so the text comes back on the
 * `runtime:clipboard` event rather than being reconstructed from what the
 * canvas was given to paint.
 */
export async function copySelection(
  anchor: { line: number; col: number },
  head: { line: number; col: number },
): Promise<void> {
  await sendRuntimeCommand({
    type: "copy_selection",
    anchor_line: anchor.line,
    anchor_col: anchor.col,
    head_line: head.line,
    head_col: head.col,
  });
}
