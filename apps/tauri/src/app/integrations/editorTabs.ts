import {
  clearEditorConflict,
  retargetEditorConflict,
} from "../../features/editor/conflict/editorConflictStore";
import { removeViews, retargetEditorViewPaths, viewsStore } from "../../navigation/viewsStore";
import { sessionIsActive, type Session } from "../../contracts/runtime";

let previousEditors = new Set<string>();

/** Session metadata is authoritative even when another client performed the move. */
export function syncEditorViewPaths(sessions: readonly Session[]): void {
  const paths = new Map<string, string>();
  for (const session of sessions) {
    if (!session.editor) continue;
    paths.set(session.id, session.editor.path);
    retargetEditorConflict(session.id, session.editor.path);
  }
  retargetEditorViewPaths(paths);
  const editors = new Set<string>();
  const ended = new Set<string>();
  for (const session of sessions) {
    if (session.kind !== "Editor") continue;
    editors.add(session.id);
    if (!sessionIsActive(session.state)) ended.add(session.id);
  }
  // A newly opened tab can arrive before the snapshot that first contains its session.
  for (const id of previousEditors) if (!editors.has(id)) ended.add(id);
  previousEditors = editors;
  for (const [workspace, views] of Object.entries(viewsStore.byWorkspace)) {
    const closed = views.open.filter(
      (view) => view.kind === "editor-terminal" && ended.has(view.session),
    );
    for (const view of closed) {
      if (view.kind === "editor-terminal") clearEditorConflict(view.session);
    }
    if (closed.length) removeViews(workspace, closed);
  }
}
