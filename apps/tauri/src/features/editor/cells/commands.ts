import type { KeyPress } from "../../../contracts/terminal";
import { sendRuntimeCommand } from "../../../runtime/host";
import { connectionStore } from "../../../state/connection";

export async function openTerminalEditor(
  workspace: string,
  path: string,
  line?: number,
  autosave = false,
): Promise<void> {
  await sendRuntimeCommand({ type: "open_editor", workspace, path, line: line ?? null, autosave });
}

export async function reopenTerminalEditor(sessionId: string): Promise<void> {
  await sendRuntimeCommand({ type: "reopen_editor", session_id: sessionId });
}

export async function sendEditorKey(session: string, key: KeyPress, id: number): Promise<void> {
  await sendRuntimeCommand({ type: "input_editor", session_id: session, key, id });
}

export async function sendEditorText(session: string, text: string, id: number): Promise<void> {
  await sendRuntimeCommand({ type: "input_editor_text", session_id: session, text, id });
}

export async function sendEditorPaste(session: string, text: string, id: number): Promise<void> {
  await sendRuntimeCommand({ type: "paste_editor", session_id: session, text, id });
}

export async function pasteEditorClipboard(
  session: string,
  read: () => Promise<string>,
  id: () => number,
): Promise<void> {
  const generation = connectionStore.connectionGeneration;
  const text = await read();
  if (!text || generation !== connectionStore.connectionGeneration) return;
  await sendEditorPaste(session, text, id());
}

/**
 * A mouse event for a Code editor pane whose `forge-editor` is reading the
 * mouse.
 *
 * The editor sibling of {@link sendMouse}: only sent while the editor's grid
 * reports a mouse mode. Unlike the terminal, `shift` is forwarded rather than
 * kept for local selection — the editor reads shift-click as an extend.
 */
export async function sendEditorMouse(
  session: string,
  event: {
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
  },
): Promise<void> {
  await sendRuntimeCommand({ type: "mouse_editor", session_id: session, ...event });
}

export async function resizeEditor(
  session: string,
  cols: number,
  rows: number,
  pixelWidth: number,
  pixelHeight: number,
): Promise<void> {
  await sendRuntimeCommand({
    type: "resize_editor",
    session_id: session,
    size: { cols, rows, pixel_width: pixelWidth, pixel_height: pixelHeight },
  });
}

/**
 * Ask for an editor's whole viewport again.
 *
 * Several files share one Code pane, so switching sub-tabs is client-side view
 * state that reaches no terminal. The freshly-shown pane asks for the frame it
 * would otherwise wait on output for — and an idle editor produces none.
 */
export async function repaintEditor(session: string): Promise<void> {
  await sendRuntimeCommand({ type: "repaint_editor", session_id: session });
}

/** Detach the Code pane; the editor process survives. */
export async function closeEditor(session: string): Promise<void> {
  await sendRuntimeCommand({ type: "close_editor", session_id: session });
}
