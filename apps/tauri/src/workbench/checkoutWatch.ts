// Keeping the Files panel honest when git moves the checkout under it.
//
// `gitSync` re-reads the rail's status at the right moments but publishes only
// `branch` + counts, and a `git pull` moves HEAD without renaming the branch.
// `WorkspaceStatus.head` (the OID from `# branch.oid`) is what separates a pull
// from a no-event; `dirty` is the second signal, covering `git stash` and
// `git restore`, which move the working tree without moving HEAD. When either
// changes since the last look, the tree and the diff are re-read in place.
//
// The re-read must not null `workbenchStore.tree`: the null between checkouts
// is what resets the panel's folds, so folding shut on every pull is the bug
// this avoids. Replacing the tree while it stays non-null keeps them.

import { createEffect } from "solid-js";
import { beginWorkbenchRequest, failWorkbenchRequest, loadFileTree } from "./api";
import { refreshDiff } from "./decorations";
import { forgeStore } from "../store/forgeStore";
import { workbenchStore } from "../store/workbenchStore";

export type CheckoutMark = {
  head: string | null;
  dirty: boolean;
};

/**
 * Whether git moved under the checkout since `before` was taken.
 *
 * An unmeasured status (`head: null`) says nothing about movement — treating
 * it as one would re-read the tree on every startup — and neither does the
 * first look, where there is nothing old to throw away.
 */
export function checkoutMoved(before: CheckoutMark | null, after: CheckoutMark | null): boolean {
  if (!after || after.head === null) return false;
  if (!before) return false;
  return before.head !== after.head || before.dirty !== after.dirty;
}

/** A burst of snapshots — one commit, one pull — is one re-read. */
const COALESCE_MS = 150;

/**
 * Re-read the tree and the diff when the active checkout's status changes.
 *
 * Marks are remembered per workspace id, so switching away and back is not a
 * movement; a status that changed while away is. Returns the teardown.
 */
export function startCheckoutWatch(): () => void {
  const marks = new Map<string, CheckoutMark>();
  let timer: ReturnType<typeof setTimeout> | undefined;

  function rereadTree(): void {
    // Re-resolved at fire time: the checkout on screen during the wait may
    // not be the one the movement was seen in.
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    beginWorkbenchRequest("tree");
    void loadFileTree(workspace).catch((error) => failWorkbenchRequest("tree", error));
  }

  createEffect(() => {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    const ws = forgeStore.workspaces.find((item) => item.id === workspace);
    if (!ws) return;
    const after: CheckoutMark = { head: ws.status.head, dirty: ws.status.dirty };
    const before = marks.get(workspace) ?? null;
    marks.set(workspace, after);
    if (!checkoutMoved(before, after)) return;
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      rereadTree();
    }, COALESCE_MS);
    refreshDiff();
  });

  return () => {
    clearTimeout(timer);
  };
}
