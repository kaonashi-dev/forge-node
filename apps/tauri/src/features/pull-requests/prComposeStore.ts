import { createStore } from "solid-js/store";
import { adoptLaunched } from "./prReview";
import { forgeStore } from "../../state/forgeStore";
import { sessionIsActive, type SessionState } from "../../contracts/runtime";

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
  state: SessionState;
};

/** What `forgectl pr draft` hands back; mirrors `domain::PullRequestDraft`. */
export type PrDraft = {
  title: string;
  body: string;
  branch: string;
  commit_message: string;
};

export type ReceivedDraft = { workspace: string; session: string; draft: PrDraft };

const [store, setStore] = createStore<{
  run: PrComposeRun | null;
  draft: ReceivedDraft | null;
  /** The workspace whose commit + push + `gh` is in flight. */
  opening: string | null;
}>({ run: null, draft: null, opening: null });

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

/**
 * Whether `run` still holds the launch button.
 *
 * Only while it is starting or its agent is alive: a session that exited, or
 * that the user closed, must free the checkout for the next task.
 */
export function composeBusy(run: PrComposeRun | null, sessions: readonly SessionRow[]): boolean {
  if (!run) return false;
  if (!run.session) return true;
  const session = sessions.find((item) => item.id === run.session);
  return session !== undefined && sessionIsActive(session.state);
}

/** The draft is not stored by the daemon; this is the only copy until it is used or dropped. */
export function receiveDraft(received: ReceivedDraft): void {
  setStore("draft", received);
}

export function clearDraft(): void {
  setStore("draft", null);
}

export function beginOpening(workspace: string): void {
  setStore("opening", workspace);
}

/** Settle an open for `workspace`; a success also consumes its draft. */
export function finishOpening(workspace: string, opened: boolean): void {
  if (store.opening === workspace) setStore("opening", null);
  if (opened && store.draft?.workspace === workspace) setStore("draft", null);
}
