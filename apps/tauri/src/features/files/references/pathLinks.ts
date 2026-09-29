// Turning a path or address named in output into an open tab or a browser.
//
// The impure half of `./pathref`: it reads the checkout the window is pointed
// at and the navigation index already loaded, and it is the one place a reference in
// the terminal or in an agent's transcript becomes a workbench tab. An address
// leaves the workbench; the platform opens it.

import { forgeStore } from "../../../state/forgeStore";
import { openUrl } from "../../../runtime/host";
import { openEditor, openEditorAt } from "../../editor/open";
import { buildPathIndex, resolvePath, type PathIndex, type PathRef } from "./pathref";
import { assembleLinks, type ScreenLink } from "./screenLinks";
import { navigationFileIndex } from "../index/fileIndex";
import { openFile, warmFileTree } from "../commands";
import { activeWorkspace, focusWorkspace } from "../../../state/workspace";

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

/** Paths and addresses on one line, resolved against the checkouts we know. */
export function linksOnLine(text: string): ScreenLink[] {
  warmPathIndex();
  const root = workspaceRoot();
  return assembleLinks(text, root, otherRoots(root), pathIndex());
}

/**
 * The file references on a line that a checkout can actually open.
 *
 * `path` comes back rewritten when the listing resolved it — a bare
 * `App.tsx` in prose, a `b/` from a diff — so the caller opens what was found
 * rather than what was written.
 */
export function linkedRefs(text: string): PathRef[] {
  const refs: PathRef[] = [];
  for (const link of linksOnLine(text)) {
    if (link.kind !== "path") continue;
    refs.push({
      from: link.from,
      to: link.to,
      path: link.path,
      line: link.line,
      checkout: link.checkout,
    });
  }
  return refs;
}

export function openScreenLink(link: ScreenLink): void {
  if (link.kind === "url") {
    void openUrl(link.url).catch(() => undefined);
    return;
  }
  openPathRef(link);
}

export function openPathRef(ref: Pick<PathRef, "path" | "line" | "checkout">): void {
  const workspace = ref.checkout === null ? activeWorkspace() : workspaceId(ref.checkout);
  if (!workspace) return;
  if (workspace !== activeWorkspace()) focusWorkspace(workspace);
  // A path into another checkout is already relative to it. Re-resolving
  // against this window's listing would rename it to a same-named file here.
  const path = ref.checkout === null ? (resolvePath(ref.path, pathIndex()) ?? ref.path) : ref.path;
  // The read is started here as well as by the editor's own effect, so the tab
  // lands on the line rather than on an empty buffer that then jumps.
  if (ref.line === null) openEditor(path);
  else openEditorAt(path, ref.line);
  void openFile(workspace, path).catch(() => undefined);
}

function otherRoots(active: string | null): string[] {
  const roots: string[] = [];
  for (const workspace of forgeStore.workspaces) {
    if (workspace.path && workspace.path !== active) roots.push(workspace.path);
  }
  return roots;
}

function workspaceId(root: string): string | null {
  const wanted = stripSlash(root);
  for (const workspace of forgeStore.workspaces) {
    if (stripSlash(workspace.path) === wanted) return workspace.id;
  }
  return null;
}

function stripSlash(path: string): string {
  return path.length > 1 ? path.replace(/\/+$/, "") : path;
}
