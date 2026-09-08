import { createStore } from "solid-js/store";
import type { ShareAction, ShareCandidate, ShareStatusEntry, ShareTrigger } from "../runtime/types";

/**
 * What the settings section knows that the snapshot does not (§14.2).
 *
 * The rules themselves live in `forgeStore.worktree_shares`, because they are
 * daemon state and the daemon is authoritative. Everything here is a *read*:
 * what the scan found, what a plan would do, and what each workspace looks
 * like right now — the same runtime-only shape as a diff or a rebase state.
 */
export type SharesState = {
  /** Detected candidates, per project id. */
  candidates: Record<string, ShareCandidate[]>;
  /** The scan stopped at its budget, per project id. */
  truncated: Record<string, boolean>;
  /** Per-rule state, per workspace id. */
  status: Record<string, ShareStatusEntry[]>;
  /** The last plan or apply result, per workspace id. */
  actions: Record<string, ShareAction[]>;
  /** Workspaces with provisioning in flight. */
  applying: string[];
  /** Why the last run happened, per workspace id. */
  trigger: Record<string, ShareTrigger>;
  scanning: boolean;
  error: string | null;
};

const [sharesStore, setSharesStore] = createStore<SharesState>({
  candidates: {},
  truncated: {},
  status: {},
  actions: {},
  applying: [],
  trigger: {},
  scanning: false,
  error: null,
});

export { sharesStore, setSharesStore };

export function applyCandidates(
  project: string,
  candidates: ShareCandidate[],
  truncated: boolean,
): void {
  setSharesStore("candidates", project, candidates);
  setSharesStore("truncated", project, truncated);
  setSharesStore({ scanning: false, error: null });
}

export function applyShareStatus(workspace: string, entries: ShareStatusEntry[]): void {
  setSharesStore("status", workspace, entries);
}

export function applyShareActions(workspace: string, actions: ShareAction[]): void {
  setSharesStore("actions", workspace, actions);
}

/** A run started: the workspace is provisioning until its event lands. */
export function markApplying(workspace: string): void {
  if (sharesStore.applying.includes(workspace)) return;
  setSharesStore("applying", [...sharesStore.applying, workspace]);
}

export function applySharesApplied(
  workspace: string,
  trigger: ShareTrigger,
  actions: ShareAction[],
  error: string | null,
): void {
  setSharesStore(
    "applying",
    sharesStore.applying.filter((id) => id !== workspace),
  );
  setSharesStore("actions", workspace, actions);
  setSharesStore("trigger", workspace, trigger);
  if (error) setSharesStore("error", error);
}

export function setSharesError(error: string | null): void {
  setSharesStore({ error, scanning: false });
}
