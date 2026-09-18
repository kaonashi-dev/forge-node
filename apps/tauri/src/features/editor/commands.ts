import type { EditorInputEvent } from "../../contracts/editor";
import { sendRuntimeCommand } from "../../runtime/host";
import { sendWorkbenchCommand } from "../../runtime/workbench";

export async function setEditorAutosave(session: string, autosave: boolean): Promise<void> {
  await sendWorkbenchCommand({ type: "set_editor_autosave", session, autosave });
}

/**
 * A burst of input for a DOM editor surface.
 *
 * Batched by the caller: one call per burst, never one per key. The daemon
 * clamps the batch, so a surface that stopped draining is truncated at the
 * boundary rather than deciding how long the host's loop runs.
 */
export async function sendEditorSurfaceInput(
  session: string,
  events: EditorInputEvent[],
): Promise<void> {
  if (events.length === 0) return;
  await sendRuntimeCommand({ type: "editor_surface_input", session_id: session, events });
}

/** Which lines a DOM editor surface has mounted, overscan included. */
export async function setEditorSurfaceView(
  session: string,
  firstLine: number,
  lineCount: number,
): Promise<void> {
  await sendRuntimeCommand({
    type: "editor_surface_view",
    session_id: session,
    first_line: firstLine,
    line_count: lineCount,
  });
}
