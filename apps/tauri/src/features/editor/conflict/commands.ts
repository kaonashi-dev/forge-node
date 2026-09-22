import { sendWorkbenchCommand } from "../../../runtime/workbench";
import { failEditorConflict, startEditorConflictLoad } from "./editorConflictStore";

/** The two sides of a refused editor save, for the conflict banner. */
export async function loadEditorConflict(session: string): Promise<void> {
  startEditorConflictLoad(session);
  try {
    await sendWorkbenchCommand({ type: "load_editor_conflict", session });
  } catch (error) {
    failEditorConflict(session, String(error));
    throw error;
  }
}

/** Take disk: replace the editor's buffer with what is on disk now. */
export async function reloadEditorBuffer(session: string): Promise<void> {
  await sendWorkbenchCommand({ type: "reload_editor_buffer", session });
}

/** Keep mine: write the editor's draft over what is on disk now. */
export async function overwriteEditorBuffer(session: string): Promise<void> {
  await sendWorkbenchCommand({ type: "overwrite_editor_buffer", session });
}
