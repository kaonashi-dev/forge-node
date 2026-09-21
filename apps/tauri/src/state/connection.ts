import { createSignal } from "solid-js";
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

/** A launch has no id to wait for: the daemon mints it. */
type PendingSelection = { id: string | null };

const [sessionSelection, setSessionSelection] = createSignal<PendingSelection | null>(null);

export const pendingSessionSelection = () => sessionSelection()?.id ?? null;

/** Whether `activeSession` is still catching up with a selection or a launch. */
export const sessionSelectionPending = () => sessionSelection() !== null;

/**
 * Mark the active session as in flight until the daemon confirms it.
 *
 * The second condition is load-bearing: with an earlier selection still
 * pending, a request for the session the store *shows* is a queued return, and
 * the intermediate payload it will pass through is not a pane the user ever
 * sat on. Without it that payload lands in the focus ring as "the tab you just
 * left". The host answers a selection of the session it is already on with
 * silence, so the pending is settled by an earlier command's payload, by the
 * refusal notice, or by a reconnection — never by one of its own.
 */
export function beginSessionSelection(id: string): () => void {
  if (id === connectionStore.activeSession && sessionSelection() === null) return () => {};
  const selection = { id };
  setSessionSelection(selection);
  return () => setSessionSelection((current) => (current === selection ? null : current));
}

/**
 * Open a session that does not exist yet.
 *
 * `showSession()` runs before the daemon has minted the id, so there is no id
 * to match on the way back: any state payload is the answer to this one.
 */
export function beginSessionLaunch(): () => void {
  const selection: PendingSelection = { id: null };
  setSessionSelection(selection);
  return () => setSessionSelection((current) => (current === selection ? null : current));
}

export function settleSessionSelection(id: string | null): void {
  setSessionSelection((current) =>
    current === null || current.id === null || current.id === id ? null : current,
  );
}

/**
 * Give up on the pending selection.
 *
 * Deliberately blunt: a refused command arrives as a `runtime:notice` rather
 * than as a rejected promise, so there is no way to tell from here whether the
 * refusal was ours. Clearing early only costs one stale focus-ring entry;
 * not clearing would suppress the ring for the rest of the session.
 */
export function clearSessionSelection(): void {
  setSessionSelection(null);
}

export function setNotice(notice: string | null): void {
  setConnectionStore("notice", notice);
}
