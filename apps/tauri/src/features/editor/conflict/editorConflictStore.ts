// The two sides of a refused editor save, keyed by session.
//
// Its own store, like `sessionChangesStore` and for the same reason: a conflict
// belongs to one editor session and has to survive switching tabs, while the
// workbench answer stores are about the checkout on screen and clear when the
// focus moves.
//
// The texts are fetched on demand — `Session.editor.conflict` is the flag that
// says there is something to fetch. They never ride `SessionUpdated`, because
// two documents on the broadcast channel are exactly what its bound is for.

import { createStore } from "solid-js/store";
import { forgeStore } from "../../../state/forgeStore";

export type EditorConflict = {
  path: string;
  /** What is on disk now. */
  disk: string;
  /** The draft the editor asked to write. */
  mine: string;
};

type Entry = {
  conflict: EditorConflict | null;
  error: string | null;
  loading: boolean;
};

const EMPTY: Entry = { conflict: null, error: null, loading: false };

const [store, setStore] = createStore<{ bySession: Record<string, Entry> }>({ bySession: {} });

export function editorConflictFor(session: string): Entry {
  return store.bySession[session] ?? EMPTY;
}

/** Mark a fetch as out, so the banner can say it is working. */
export function startEditorConflictLoad(session: string): void {
  setStore("bySession", session, { ...editorConflictFor(session), loading: true, error: null });
}

export function applyEditorConflict(session: string, conflict: EditorConflict): void {
  const path =
    forgeStore.sessions.find((item) => item.id === session)?.editor?.path ?? conflict.path;
  setStore("bySession", session, { conflict: { ...conflict, path }, error: null, loading: false });
}

export function retargetEditorConflict(session: string, path: string): void {
  const conflict = store.bySession[session]?.conflict;
  if (conflict && conflict.path !== path) setStore("bySession", session, "conflict", "path", path);
}

export function failEditorConflict(session: string, error: string): void {
  setStore("bySession", session, { conflict: null, error, loading: false });
}

/**
 * Drop what was held for a session.
 *
 * Called when the flag clears — the save went through or the buffer was
 * reloaded — so a banner cannot outlive the conflict that raised it.
 */
export function clearEditorConflict(session: string): void {
  setStore("bySession", session, undefined!);
}
