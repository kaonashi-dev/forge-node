// Turning a path named in output into an open tab.
//
// The impure half of `./pathref`: it reads the checkout the window is pointed
// at and the file tree already in hand, and it is the one place a reference in
// the terminal or in an agent's transcript becomes a workbench tab.

import { forgeStore } from "../store/forgeStore";
import { openEditor, openEditorAt } from "../store/viewsStore";
import { workbenchStore } from "../store/workbenchStore";
import { openFile, warmFileTree } from "./api";
import { buildPathIndex, findPathRefs, resolvePath, type PathIndex, type PathRef } from "./pathref";

/** The absolute path of the checkout, which is how output spells it. */
export function workspaceRoot(): string | null {
  const id = workbenchStore.workspace;
  if (!id) return null;
  return forgeStore.workspaces.find((workspace) => workspace.id === id)?.path ?? null;
}

let cache: { entries: unknown; index: PathIndex } | null = null;

/**
 * What the loaded tree can say about a guessed path, or `null` for "nothing
 * read this checkout".
 *
 * A truncated listing counts as nothing: the scan hit its budget, so a file
 * missing from it is not evidence that the file is missing.
 *
 * Rebuilt only when the tree itself is replaced. The listing is thousands of
 * entries and this is read per hovered cell.
 */
export function pathIndex(): PathIndex | null {
  const tree = workbenchStore.tree;
  if (!tree || tree.truncated || tree.workspace_id !== workbenchStore.workspace) return null;
  if (cache?.entries !== tree.entries) {
    cache = { entries: tree.entries, index: buildPathIndex(tree.entries) };
  }
  return cache.index;
}

/** Ask for the listing the hover accuracy above depends on. */
export function warmPathIndex(): void {
  const workspace = workbenchStore.workspace;
  if (workspace) warmFileTree(workspace);
}

/**
 * The references on a line that this checkout can actually open.
 *
 * `path` comes back rewritten when the listing resolved it — a bare
 * `App.tsx` in prose, a `b/` from a diff — so the caller opens what was found
 * rather than what was written.
 */
export function linkedRefs(text: string): PathRef[] {
  const root = workspaceRoot();
  const index = pathIndex();
  const linked: PathRef[] = [];
  for (const ref of findPathRefs(text, root)) {
    const path = resolvePath(ref.path, index);
    if (path !== null) linked.push(path === ref.path ? ref : { ...ref, path });
  }
  return linked;
}

export function openPathRef(ref: PathRef): void {
  const workspace = workbenchStore.workspace;
  if (!workspace) return;
  const path = resolvePath(ref.path, pathIndex()) ?? ref.path;
  // Same pair as the diff's D4 jump: the read is started here as well as by the
  // editor's own effect, so the tab lands on the line rather than on an empty
  // buffer that then jumps.
  if (ref.line === null) openEditor(path);
  else openEditorAt(path, ref.line);
  void openFile(workspace, path).catch(() => undefined);
}
