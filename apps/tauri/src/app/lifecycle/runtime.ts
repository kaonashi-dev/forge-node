import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { applyConnected, applyStatePayload } from "./connection";
import { disconnectFileWatches } from "../../features/files/watches/fileWatch";
import { directories } from "../../features/files/directories/directoryState";
import { pathOperations } from "../../features/files/operations/operations";
import { clearSessionSelection, setConnectionStore, setNotice } from "../../state/connection";
import type { ConnectedPayload, DisconnectedPayload, StatePayload } from "../../contracts/runtime";
import { setFilesStore } from "../../features/files/state";

export function bindRuntimeEvents(): Promise<UnlistenFn[]> {
  return Promise.all([
    listen("runtime:connecting", () => {
      clearSessionSelection();
      pathOperations.disconnect();
      directories.connection(false);
      disconnectFileWatches();
      setConnectionStore("connection", { kind: "connecting" });
    }),
    listen<ConnectedPayload>("runtime:connected", (event) => {
      applyConnected(event.payload);
    }),
    listen<StatePayload>("runtime:state", (event) => {
      applyStatePayload(event.payload);
    }),
    listen<{ reason: string }>("runtime:notice", (event) => {
      clearSessionSelection();
      setNotice(event.payload.reason);
    }),
    listen<DisconnectedPayload>("runtime:disconnected", (event) => {
      clearSessionSelection();
      setFilesStore("treeRequest", null);
      pathOperations.disconnect();
      directories.connection(false);
      disconnectFileWatches();
      setConnectionStore("connection", {
        kind: "disconnected",
        reason: event.payload.reason,
      });
    }),
  ]);
}
