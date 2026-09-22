import { batch } from "solid-js";
import { reconcile } from "solid-js/store";
import { applyShellSnapshot, forgeStore } from "../../state/forgeStore";
import { adoptPendingCompose } from "../../features/pull-requests/prComposeStore";
import { adoptPendingReviews } from "../../features/pull-requests/prReviewStore";
import { syncHandoffSession } from "../../features/sessions/handoffJobStore";
import {
  clearSessionSelection,
  setConnectionStore,
  settleSessionSelection,
} from "../../state/connection";
import { invalidateFileIndex, setFilesStore } from "../../features/files/state";
import { syncEditorViewPaths } from "../integrations/editorTabs";
import { createEditorAutosaveSync } from "../../features/editor/editorAutosave";
import { AUTOSAVE_KEY, readFlag } from "../../state/preferences";
import { setEditorAutosave } from "../../features/editor/commands";
import { setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";
import { directories } from "../../features/files/directories/directoryState";
import { pathOperations } from "../../features/files/operations/operations";
import { parentPath } from "../../shared/paths";
import { reconnectFileWatches } from "../../features/files/watches/fileWatch";
import type { ConnectedPayload, StatePayload } from "../../contracts/runtime";

const syncEditorAutosave = createEditorAutosaveSync(setEditorAutosave);

export function applyConnected(payload: ConnectedPayload): void {
  clearSessionSelection();
  applyShellSnapshot(payload.store);
  syncEditorViewPaths(forgeStore.sessions);
  syncEditorAutosave(forgeStore.sessions, readFlag(AUTOSAVE_KEY, false), true);
  adoptPendingLaunches();

  // A workbench command queued across a drop is answered by nothing, and its
  // `loading` flag is what every one of these reads is guarded by: left up, the
  // tree, the file and the decorations are never asked for again. The reads are
  // idempotent and their surfaces re-ask on the next effect run.
  batch(() => {
    setLoading(reconcile({}));
    setFilesStore("treeRequest", null);
    const workspace = activeWorkspace();
    if (workspace) invalidateFileIndex(workspace);
  });
  setConnectionStore({
    connectionGeneration: payload.connection_generation,
    connection: {
      kind: "connected",
      instanceId: payload.daemon.instance_id,
      version: payload.daemon.daemon_version,
      editorSurface: payload.daemon.editor_surface,
    },
    activeSession: payload.active_session,
    activeTerminal: payload.active_terminal,
  });
  directories.connection(true);
  reconnectFileWatches();
  pathOperations.reconcile(
    (operation) => {
      for (const path of [operation.from, operation.to]) {
        if (path !== undefined) directories.ensure(operation.workspace, parentPath(path), true);
      }
    },
    () => {
      const workspace = activeWorkspace();
      if (workspace) directories.invalidate(workspace);
    },
  );
}

/** Agent launches that outlive their tab: bind them once the snapshot lands. */
function adoptPendingLaunches(): void {
  adoptPendingReviews(forgeStore.sessions);
  adoptPendingCompose(forgeStore.sessions);
  syncHandoffSession();
}

export function applyStatePayload(payload: StatePayload): void {
  applyShellSnapshot(payload.store);
  syncEditorViewPaths(forgeStore.sessions);
  syncEditorAutosave(forgeStore.sessions, readFlag(AUTOSAVE_KEY, false));
  adoptPendingLaunches();
  batch(() => {
    setConnectionStore("activeSession", payload.active_session);
    setConnectionStore("activeTerminal", payload.active_terminal);
    settleSessionSelection(payload.active_session);
  });
}
