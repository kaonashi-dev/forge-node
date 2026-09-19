import { open } from "../../navigation/viewsStore";
import { AUTOSAVE_KEY, readFlag } from "../../state/preferences";
import { activeWorkspace } from "../../state/workspace";
import { noteFileOpened } from "../files/index/recentFiles";
import { opensInEditor } from "../files/preview/previewRoute";
import { openTerminalEditor } from "./cells/commands";

/**
 * Open a file on whichever surface owns it.
 *
 * Text goes to the terminal editor: the daemon creates (or re-uses) the
 * session and the `EditorOpened` event opens the tab, so there is no view to
 * add here. What stays on the DOM side is the rendered kinds a TUI cannot
 * draw — Markdown, SVG, a raster image — which `editorRouteFor` decides.
 *
 * `line` rides along so the daemon can reveal it at creation; a jump into a
 * file that is already open becomes a `Reveal` on the live session rather than
 * a second editor process.
 */
export function openEditor(path: string, line?: number): void {
  // Opening is the whole of what "recent" means here — the tree carries no
  // mtime, so this is where the palette's opening list comes from. Hooked at
  // the one choke point rather than at each caller, so the file tree, a
  // definition jump and a diff all count the same as the palette itself.
  noteFileOpened(activeWorkspace(), path);
  const workspace = activeWorkspace();
  if (!workspace) return;
  // The tab arrives with `EditorOpened`; a failure leaves the current view
  // alone rather than opening an empty one. The autosave preference travels
  // with the open: the daemon holds no opinion about it and the editor process
  // is what acts on it.
  // A rendered kind never reaches the daemon's editor: it is a read the DOM
  // draws, so the tab is opened here rather than waiting for `EditorOpened`.
  if (!opensInEditor(path)) {
    open({ kind: "preview", path });
    return;
  }
  void openTerminalEditor(workspace, path, line, readFlag(AUTOSAVE_KEY, false)).catch(
    () => undefined,
  );
}

/**
 * Open a file *at* a line: a diff hunk, a review note, a definition.
 *
 * The line is a request that stands until the editor honours it, not part of
 * the view — `viewKey` is the path alone, so a second jump into a file already
 * open moves the caret in the tab that is there rather than opening a rival
 * one, and a parked tab reopened later is not stuck on an old line.
 *
 * No `revealInTree`: the Files panel follows the active view on its own
 * (`workbench/treeFollow.ts`), and following keeps the filter the person is
 * working in — an explicit reveal would drop it for a jump that did not ask.
 */
export function openEditorAt(path: string, line: number): void {
  openEditor(path, line);
}
