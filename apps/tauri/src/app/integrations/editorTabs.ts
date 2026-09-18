import { retargetEditorConflict } from "../../features/editor/conflict/editorConflictStore";
import { retargetEditorViewPaths } from "../../navigation/viewsStore";
import type { Session } from "../../contracts/runtime";

/** Session metadata is authoritative even when another client performed the move. */
export function syncEditorViewPaths(sessions: readonly Session[]): void {
  const paths = new Map<string, string>();
  for (const session of sessions) {
    if (!session.editor) continue;
    paths.set(session.id, session.editor.path);
    retargetEditorConflict(session.id, session.editor.path);
  }
  retargetEditorViewPaths(paths);
}
