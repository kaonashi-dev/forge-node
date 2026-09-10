import { createStore } from "solid-js/store";
import { adoptLaunched } from "../workbench/prReview";
import { forgeStore } from "./forgeStore";

/**
 * The agent review in flight for each pull request (§16.9).
 *
 * Outside the tab, because the tab is unmounted the moment you look at
 * something else: kept in the component, the running session would vanish from
 * the screen on the way to the diff and come back offering to start a second
 * one. Keyed on the pull request for the same reason the tab is — one review
 * per pull request, and pressing Review again finds the one already going.
 *
 * Window state, not daemon state. A restart of the app forgets which session
 * was a review; the session itself survives in the rail, tagged `Reviewer`.
 *
 * Adoption also lives here rather than in the tab: the runtime command channel
 * is one-way, so the session id never comes back with the launch, and the tab
 * is often gone by the time the session appears in a snapshot. Matching on
 * "an agent session in this checkout, created after we asked" has to keep
 * running while the reader is elsewhere, or the UI stays on Starting… forever.
 */
export type PrReviewRun = {
  /**
   * When the launch was asked for.
   *
   * The runtime command channel is one-way, so the id of the session it created
   * does not come back; this is what tells the adoption which sessions are new
   * enough to be the one.
   */
  startedAt: string;
  /** The adopted session, once one has appeared. */
  session: string | null;
  /** The checkout it was started in, so the adoption looks in the right one. */
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

/** Record that a launch has been asked for, before it can possibly land. */
export function beginReview(key: string, workspace: string): void {
  const startedAt = new Date().toISOString();
  setStore(key, { startedAt, session: null, workspace });
  // A session that starts in the same tick as the ask is still newer than the
  // floor we just wrote; adopt immediately so a fast daemon does not wait on
  // the next snapshot the reader may never see while the tab is closed.
  adoptPendingReviews(forgeStore.sessions);
}

/**
 * Bind every open launch to the session it produced, if one has landed.
 *
 * Called from the snapshot path so adoption does not depend on a mounted tab.
 */
export function adoptPendingReviews(sessions: readonly SessionRow[]): void {
  for (const key of Object.keys(store)) {
    const run = store[key];
    if (!run || run.session) continue;
    const found = adoptLaunched(sessions, run.workspace, run.startedAt);
    if (found) setStore(key, "session", found.id);
  }
}

/** Forget the run — a launch the daemon refused, or a second pass replacing it. */
export function clearReview(key: string): void {
  setStore(key, undefined!);
}
