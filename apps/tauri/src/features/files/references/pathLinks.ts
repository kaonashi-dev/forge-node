// Turning a path named in output into an open tab.
//
// The impure half of `./pathref`: it reads the checkout the window is pointed
// at and the file tree already in hand, and it is the one place a reference in
// the terminal or in an agent's transcript becomes a workbench tab.

import { forgeStore } from "../../../state/forgeStore";
import { openEditor, openEditorAt } from "../../editor/open";
import { buildPathIndex, findPathRefs, resolvePath, type PathIndex, type PathRef } from "./pathref";
import { navigationFileIndex } from "../index/fileIndex";
import { openFile, warmFileTree } from "../commands";
import { activeWorkspace } from "../../../state/workspace";

/** The absolute path of the checkout, which is how output spells it. */
export function workspaceRoot(): string | null {
  const id = activeWorkspace();
  if (!id) return null;
  return forgeStore.workspaces.find((workspace) => workspace.id === id)?.path ?? null;
}

let cache: { tree: unknown; index: PathIndex } | null = null;

/** Cached by listing identity because terminal hover reads it per cell. */
export function pathIndex(): PathIndex | null {
  const tree = navigationFileIndex();
  if (!tree || tree.workspace_id !== activeWorkspace()) return null;
  if (cache?.tree !== tree) {
    cache = { tree, index: buildPathIndex(tree.entries, !tree.truncated) };
  }
  return cache.index;
}

/** Ask for the listing the hover accuracy above depends on. */
export function warmPathIndex(): void {
  const workspace = activeWorkspace();
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
  warmPathIndex();
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
  const workspace = activeWorkspace();
  if (!workspace) return;
  const path = resolvePath(ref.path, pathIndex()) ?? ref.path;
  // Same pair as the diff's D4 jump: the read is started here as well as by the
  // editor's own effect, so the tab lands on the line rather than on an empty
  // buffer that then jumps.
  if (ref.line === null) openEditor(path);
  else openEditorAt(path, ref.line);
  void openFile(workspace, path).catch(() => undefined);
}
