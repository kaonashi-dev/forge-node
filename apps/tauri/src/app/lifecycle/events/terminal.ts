import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cellsChannel, clipboardChannel } from "../../../runtime/bus";
import type { CellsPayload } from "../../../contracts/terminal";

export function bindTerminalEvents(): Promise<UnlistenFn[]> {
  return Promise.all([
    // Terminal frames go straight to the canvas: see `runtime/bus.ts` for why
    // they must not pass through a Solid store.
    listen<CellsPayload>("runtime:cells", (event) => {
      cellsChannel.publish(event.payload);
    }),
    listen<{ text: string }>("runtime:clipboard", (event) => {
      clipboardChannel.publish(event.payload.text);
    }),
  ]);
}
