import { forgeStore } from "../../state/forgeStore";
import { requestConfirm } from "../../state/dialogs";
import { setNotice } from "../../state/connection";
import { setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";
import { openDiff } from "../../navigation/viewsStore";
import { newAgent } from "../sessions/commands";
import { conflictPrompt, resolverFor } from "./conflictPrompt";
import {
  abortRebase,
  continueRebase,
  loadDiff,
  loadRebaseState,
  markConflictResolved,
} from "./commands";
import { gitStore, setGitStore } from "./state";

// A stopped replay is read, never remembered: every action re-reads the
// sequencer and the working tree rather than adjusting a count kept here.

export function refreshGit(): void {
  const workspace = activeWorkspace();
  if (!workspace) return;
  setLoading("diff", true);
  setLoading("rebase", true);
  void loadDiff(workspace).catch(() => undefined);
  void loadRebaseState(workspace).catch(() => undefined);
}

function act(run: (workspace: string) => Promise<void>): void {
  const workspace = activeWorkspace();
  if (!workspace) return;
  setLoading("rebase", true);
  void run(workspace)
    .then(() => {
      setLoading("diff", true);
      return loadDiff(workspace);
    })
    .catch(() => undefined);
}

export function continueReplay(): void {
  act(continueRebase);
}

/** Staging is what accepts a resolution, so markers still in the file ask first. */
export function stageConflict(path: string, markersLeft: number): void {
  const run = () => act((workspace) => markConflictResolved(workspace, [path]));
  if (markersLeft === 0) {
    run();
    return;
  }
  requestConfirm({
    title: `Stage ${path} with conflict markers in it?`,
    description: `${markersLeft} conflict region(s) are still marked in the working tree. Staging commits the markers as text.`,
    confirmLabel: "Stage anyway",
    destructive: true,
    onConfirm: run,
  });
}

/** Aborting throws every resolution away; continuing only moves staged work. */
export function abortReplay(): void {
  const state = gitStore.rebase;
  const count = state?.conflicts.length ?? 0;
  requestConfirm({
    title: `Abort the ${state?.operation?.toLowerCase() ?? "rebase"}?`,
    description:
      count > 0
        ? `${count} unresolved path(s) will be lost, along with every resolution made so far.`
        : "Every resolution made so far is thrown away.",
    confirmLabel: "Abort",
    destructive: true,
    onConfirm: () => act(abortRebase),
  });
}

/**
 * The ordinary `CreateAgentSession`, with the conflict prompt already typed.
 *
 * An agent that cannot take a prompt is refused here with a sentence naming
 * what to change, rather than arriving at the daemon as a launch that did
 * nothing.
 */
export function resolveWithAgent(): void {
  const target = activeWorkspace();
  const state = gitStore.rebase;
  if (!target || !state) return;
  const resolver = resolverFor(forgeStore);
  if (resolver.kind === "unavailable") {
    setNotice(resolver.reason);
    return;
  }
  void newAgent(resolver.provider, resolver.profile, target, null, conflictPrompt(state)).catch(
    () => undefined,
  );
}

export function reviewConflict(path: string | null): void {
  setGitStore("conflictFocus", path);
  openDiff();
}
