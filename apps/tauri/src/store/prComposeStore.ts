import { createStore } from "solid-js/store";
import { adoptLaunched } from "../workbench/prReview";
import { forgeStore } from "./forgeStore";

/**
 * The agent launch in flight for the PR compose tab.
 *
 * Same shape as {@link ../store/prReviewStore}: the tab unmounts when the
 * reader looks elsewhere, and a local `awaiting` signal would either stick on
 * Starting… forever (if kept in a store without adoption) or vanish and allow
 * a second launch (if kept only in the component). One run at a time, keyed
 * by checkout — compose is always about the workspace on screen.
 */
export type PrComposeRun = {
  startedAt: string;
  session: string | null;
  workspace: string;
};

type SessionRow = {
  id: string;
  workspace_id: string;
  created_at: string;
  agent_provider_id: string | null;
};

const [store, setStore] = createStore<{ run: PrComposeRun | null }>({ run: null });

export const prComposeStore = store;

export function composeRun(): PrComposeRun | null {
  return store.run;
}

export function beginCompose(workspace: string): void {
  const startedAt = new Date().toISOString();
  setStore("run", { startedAt, session: null, workspace });
  adoptPendingCompose(forgeStore.sessions);
}

export function clearCompose(): void {
  setStore("run", null);
}

/** Bind the open compose launch to the session it produced, if one has landed. */
export function adoptPendingCompose(sessions: readonly SessionRow[]): void {
  const run = store.run;
  if (!run || run.session) return;
  const found = adoptLaunched(sessions, run.workspace, run.startedAt);
  if (found) setStore("run", "session", found.id);
}
