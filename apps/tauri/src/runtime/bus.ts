// Terminal frames bypass the Solid stores on purpose.
//
// A store write deep-proxies what it is given and notifies every reader; a
// frame arrives up to 62 times a second and has exactly one reader, the canvas.
// Routing it through `createStore` would put the proxy machinery on the delta
// rung for no subscriber that wanted it.

import type { CellsPayload, EditorFramePayload } from "../contracts/terminal";

type Listener<T> = (payload: T) => void;

function channel<T>() {
  const listeners = new Set<Listener<T>>();
  return {
    subscribe(listener: Listener<T>): () => void {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    publish(payload: T): void {
      for (const listener of listeners) listener(payload);
    },
    get size(): number {
      return listeners.size;
    },
  };
}

export const cellsChannel = channel<CellsPayload>();
/**
 * The harness preview's frames.
 *
 * Its own channel, not a second subscriber on `cellsChannel`: the two
 * surfaces paint different terminals, and a canvas that had to check every
 * frame's terminal id before drawing it would do that check 62 times a second
 * for the one it does not want.
 */
export const previewCellsChannel = channel<CellsPayload>();
/**
 * Frames for a daemon-supervised editor in the Code region.
 *
 * Its own channel so the hidden main pane never has to filter terminal ids
 * on the delta rung.
 */
export const editorCellsChannel = channel<CellsPayload>();
/**
 * Windows for a headless editor's DOM surface.
 *
 * Apart from `editorCellsChannel` because the two are different surfaces, not
 * two encodings of one: a frame here is lines and scopes, and a subscriber to
 * one has nothing to do with the other.
 */
export const editorFrameChannel = channel<EditorFramePayload>();
export const clipboardChannel = channel<string>();

export const fileChangesChannel = channel<[workspace: string, path: string]>();
