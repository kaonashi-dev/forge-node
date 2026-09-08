import { createEffect, on, type JSX } from "solid-js";
import { forgeStore } from "../store/forgeStore";
import { runtimeStore } from "../store/runtimeStore";
import {
  INSPECTOR_TABS,
  inspectorTab,
  setInspectorTab,
  type InspectorTab,
} from "../store/inspectorStore";
import { focusWorkspace, workbenchStore } from "../store/workbenchStore";
import { centerMode } from "../store/viewsStore";
import { FeaturesPanel } from "./FeaturesPanel";
import { FileTreePanel } from "./FileTreePanel";
import { GitPanel } from "./GitPanel";
import { HistoryPanel } from "./HistoryPanel";
import { LieutenantPanel } from "./LieutenantPanel";
import { PullRequestPanel } from "./PullRequestPanel";
import { Tabs } from "../ui";

export function RightPanel() {
  const tab = inspectorTab;

  /*
   * The panels answer questions about a checkout, and the active session is
   * what says which one. Switching sessions clears them rather than leaving
   * the previous checkout's diff under a new branch name.
   *
   * `on`, and tracking the session's workspace alone. As a bare effect this
   * also depended on what it wrote: `focusWorkspace` reads
   * `workbenchStore.workspace` to decide whether anything changed, so anyone
   * else focusing a checkout — the palette going to a workspace, the file
   * palette, the Git panel with no session open — woke this effect, which saw
   * no active session and immediately cleared it again. The checkout could not
   * be pointed anywhere the active session was not already.
   *
   * Do not guess from the workspace list while the runtime is still settling:
   * that list may contain retained worktrees whose directories no longer
   * exist, so the active session stays the authoritative selection.
   */
  createEffect(
    on(
      () =>
        forgeStore.sessions.find((item) => item.id === runtimeStore.activeSession)?.workspace_id,
      (workspace) => {
        // A new tab must not steal the Code view: while it is up the checkout
        // on screen belongs to what is being read, not to the session that
        // just started in the background. Explicit session picks leave Code
        // first (they call showSession), so by the time this reruns the mode
        // already says where to go.
        if (centerMode() === "code") return;
        focusWorkspace(workspace ?? null);
      },
    ),
  );

  const PANELS: Record<InspectorTab, () => JSX.Element> = {
    Files: () => <FileTreePanel />,
    Git: () => <GitPanel />,
    History: () => <HistoryPanel />,
    PR: () => <PullRequestPanel />,
    Features: () => <FeaturesPanel />,
    Lieutenant: () => <LieutenantPanel />,
  };

  return (
    <Tabs
      class="inspector"
      aria-label="Inspector"
      value={tab()}
      onChange={(value) => setInspectorTab(value as InspectorTab)}
      listClass="inspector-tabs"
      triggerClass="inspector-tab"
      contentClass="inspector-body"
      tabs={INSPECTOR_TABS.map((item) => ({ value: item, label: item, content: PANELS[item] }))}
    />
  );
}
