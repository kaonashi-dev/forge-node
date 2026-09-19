import { createStore } from "solid-js/store";
import type { EditorSurface } from "../contracts/runtime";

export type ConnectionState =
  | { kind: "idle" }
  | { kind: "connecting" }
  | {
      kind: "connected";
      instanceId: string;
      version: string;
      /** Which pane the Code region mounts for an editor (`[editor] surface`). */
      editorSurface: EditorSurface;
    }
  | { kind: "disconnected"; reason: string };

export const [connectionStore, setConnectionStore] = createStore({
  connection: { kind: "idle" } as ConnectionState,
  connectionGeneration: 0,
  activeSession: null as string | null,
  activeTerminal: null as string | null,
  /** Command failures do not imply a lost connection. */
  notice: null as string | null,
});

export function setNotice(notice: string | null): void {
  setConnectionStore("notice", notice);
}
