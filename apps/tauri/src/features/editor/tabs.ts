import { closeEditor } from "./cells/commands";
import { currentViews, removeViews } from "../../navigation/viewsStore";
import { sameView, type WorkbenchView } from "../../navigation/views";
import { forgeStore } from "../../state/forgeStore";
import { requestConfirm } from "../../state/dialogs";
import { activeWorkspace } from "../../state/workspace";

export function close(view: WorkbenchView): void {
  closeViews([view]);
}

function closeViews(targets: WorkbenchView[]): void {
  const workspace = activeWorkspace();
  if (!workspace) return;
  const finish = () => {
    for (const view of targets) {
      if (view.kind === "editor-terminal") closeEditor(view.session).catch(() => undefined);
    }
    removeViews(workspace, targets);
  };
  const unsaved = targets.some(
    (view) =>
      view.kind === "editor-terminal" &&
      forgeStore.sessions.find((session) => session.id === view.session)?.editor?.dirty !== false,
  );
  if (unsaved) {
    requestConfirm({
      title: "Close files with unsaved changes?",
      description: "These files have unsaved changes. Cancel to save them before closing.",
      confirmLabel: "Close without saving",
      onConfirm: finish,
    });
  } else {
    finish();
  }
}

export function closeCode(): void {
  closeViews(currentViews().open);
}

/** Close every view but the active one. */
export function closeOtherViews(): void {
  const views = currentViews();
  closeViews(views.open.filter((view) => !sameView(view, views.active)));
}

/** Close everything after the active view in the strip. */
export function closeViewsToRight(): void {
  const views = currentViews();
  const index = views.open.findIndex((item) => sameView(item, views.active));
  if (index >= 0) closeViews(views.open.slice(index + 1));
}
