// U8 / A5: keeping the git decorations fed.
//
// The tree marks and the editor's gutter stripes both read
// `workbenchStore.diff`, and until this module existed the only thing that
// ever filled it was the Git tab's `onMount`. Browse a checkout without
// opening that tab — which is the ordinary way to use the Files panel — and
// every decoration was simply absent; switch sessions and `focusWorkspace`
// nulled the diff with nothing left to restore it.
//
// So the read is asked for by the surfaces that *paint* it rather than by the
// one tab that happens to list it. `ensureDiff` is the idempotent open-a-file
// call; `refreshDiff` is what a write goes through.

import { setLoading, setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import { fileChangesChannel } from "../runtime/bus";
import { loadDiff } from "./api";

/** A burst of autosaves is one read, not one per keystroke pause. */
const COALESCE_MS = 150;

/**
 * The fastest an agent's writes may re-read the diff.
 *
 * A `git diff` of a large checkout is seconds of subprocess, and an agent
 * writing for a minute produces events for the whole minute. This is
 * `sessionChangesStore`'s `REFRESH_FLOOR_MS` applied to the same problem: the
 * marks follow the work without the work paying for the marks. A save and any
 * other explicit gesture goes through `refreshDiff`, which ignores the floor.
 */
const WATCH_FLOOR_MS = 10_000;

let pending: ReturnType<typeof setTimeout> | undefined;
let watchedAt = 0;

function read(workspace: string): void {
  setLoading("diff", true);
  setWorkbenchStore("diffError", null);
  // A failed decoration read is not worth a banner: the tree and the gutter
  // both degrade to "nothing changed here", and the Git tab reports the same
  // failure with somewhere to put it.
  void loadDiff(workspace).catch(() => setLoading("diff", false));
}

/**
 * Read the diff for the focused checkout unless there is already one.
 *
 * Guarded the way the file tree's own load is: an answer already in the store
 * is not asked for again, and two panels mounting together make one request.
 */
export function ensureDiff(): void {
  const workspace = workbenchStore.workspace;
  // `diffError` is part of the guard for the same reason it is in
  // `warmFileTree`: the callers are tracking effects that re-run when the
  // failure lands, and a read that only ever fails would otherwise be asked
  // for again on every one of them, forever. `refreshDiff` is the retry.
  if (
    !workspace ||
    workbenchStore.diff ||
    workbenchStore.loading.diff ||
    workbenchStore.diffError
  ) {
    return;
  }
  read(workspace);
}

/**
 * Read it again because something wrote to the checkout.
 *
 * Unguarded by `loading.diff` on purpose — a save that lands while a read is
 * in flight is exactly the one whose result that read does not contain — and
 * coalesced instead, so autosave cannot queue a `git diff` per pause.
 */
export function refreshDiff(): void {
  const workspace = workbenchStore.workspace;
  if (!workspace) return;
  setWorkbenchStore("diffError", null);
  clearTimeout(pending);
  pending = setTimeout(() => {
    pending = undefined;
    // Re-read rather than closing over it: the checkout may have moved during
    // the wait, and decorating it with the previous one's diff is the bug
    // `focusWorkspace` clears the store to avoid.
    const current = workbenchStore.workspace;
    if (current) read(current);
  }, COALESCE_MS);
}

/**
 * Follow the checkout, not just this window's saves.
 *
 * Without this the marks and the gutter stripes are painted from whatever the
 * diff was when the panel mounted: an agent writing forty files moves the tree
 * and the open buffer — both watch the same events — and leaves the decorations
 * on them describing the state before the work. `checkoutWatch` only catches
 * what moves `head` or flips `dirty` once, which an ordinary write does not.
 *
 * Subscribed once at module scope, like the channel it reads: the decorations
 * belong to the focused checkout and not to any one panel, and a panel-owned
 * subscription would stop following the moment that panel unmounted.
 */
fileChangesChannel.subscribe(([workspace]) => {
  if (workspace !== workbenchStore.workspace || workbenchStore.loading.diff) return;
  const now = Date.now();
  if (now - watchedAt < WATCH_FLOOR_MS) return;
  watchedAt = now;
  refreshDiff();
});
