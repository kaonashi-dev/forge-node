import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { editorCellsChannel, editorFrameChannel } from "../../../runtime/bus";
import type { CellsPayload, EditorFramePayload } from "../../../contracts/terminal";
import { openEditorTerminal } from "../../../navigation/viewsStore";

export function bindEditorEvents(): Promise<UnlistenFn[]> {
  return Promise.all([
    listen<CellsPayload>("runtime:editor_cells", (event) => {
      editorCellsChannel.publish(event.payload);
    }),
    listen<EditorFramePayload>("runtime:editor_frame", (event) => {
      editorFrameChannel.publish(event.payload);
    }),
    listen<{
      session_id: string;
      terminal_id: string;
      workspace: string;
      path: string;
    }>("runtime:editor_opened", (event) => {
      openEditorTerminal(event.payload.session_id, event.payload.path, event.payload.workspace);
    }),
  ]);
}
