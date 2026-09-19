import { createStore } from "solid-js/store";
import { adoptLaunched } from "./prReview";
import { forgeStore } from "../../state/forgeStore";

// Window-scoped so pending reviews survive tab unmounts without launching twice.
export type PrReviewRun = {
  /** Adoption cutoff: the command channel does not return a session id. */
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

const [store, setStore] = createStore<Record<string, PrReviewRun>>({});

export const prReviewStore = store;

export function reviewRun(key: string): PrReviewRun | null {
  return store[key] ?? null;
}

export function beginReview(key: string, workspace: string): void {
  const startedAt = new Date().toISOString();
  setStore(key, { startedAt, session: null, workspace });
  // A fast launch may already be in the store, with no later snapshot to adopt it.
  adoptPendingReviews(forgeStore.sessions);
}

/** Called on snapshots even when the review tab is unmounted. */
export function adoptPendingReviews(sessions: readonly SessionRow[]): void {
  for (const key of Object.keys(store)) {
    const run = store[key];
    if (!run || run.session) continue;
    const found = adoptLaunched(sessions, run.workspace, run.startedAt);
    if (found) setStore(key, "session", found.id);
  }
}

export function clearReview(key: string): void {
  setStore(key, undefined!);
}
