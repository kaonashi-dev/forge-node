// Keeping the file tree on whichever editor is active.
//
// `treeReveal` is the explicit "show me this path" request; this is the
// passive direction, and it has to answer every way an editor can become
// active — a tab click, the palette, a path link, a close falling back to a
// neighbour, a parked view restored on a workspace switch — which is why it
// watches the active view instead of being pushed by each opener.

import { createEffect, createMemo } from "solid-js";
import type { FileTree } from "../../../contracts/workbench";
import { activeEditorPath, type WorkbenchView } from "../../../navigation/views";

/**
 * Follow the active editor once both the panel and the listing are there.
 *
 * The effect re-runs when the sidebar mounts or the listing lands, so a path
 * chosen while either was missing is not lost. There is deliberately no
 * preference behind it: the panel is opinionated, and a toggle can be added if
 * anyone asks.
 */
export function installTreeFollow(host: {
  mounted: () => boolean;
  tree: () => FileTree | null;
  view: () => WorkbenchView;
  follow: (path: string) => void;
}): void {
  // A memo on the path alone: a store write that leaves the active editor
  // where it is must not re-run the effect and re-scroll the tree.
  const path = createMemo(() => activeEditorPath(host.view()));
  // Readiness, not identity: a re-listing replaces the tree object without
  // making the active editor a different one, and following again would yank
  // the list back from wherever the person has scrolled.
  const loaded = createMemo(() => host.tree() !== null);
  createEffect(() => {
    if (!host.mounted() || !loaded()) return;
    const wanted = path();
    if (wanted !== null) host.follow(wanted);
  });
}
