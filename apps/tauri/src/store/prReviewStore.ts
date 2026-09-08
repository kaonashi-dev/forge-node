import { createStore } from "solid-js/store";

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

const [store, setStore] = createStore<Record<string, PrReviewRun>>({});

export const prReviewStore = store;

export function reviewRun(key: string): PrReviewRun | null {
  return store[key] ?? null;
}

/** Record that a launch has been asked for, before it can possibly land. */
export function beginReview(key: string, workspace: string): void {
  setStore(key, { startedAt: new Date().toISOString(), session: null, workspace });
}

export function adoptReviewSession(key: string, session: string): void {
  if (store[key]) setStore(key, "session", session);
}

/** Forget the run — a launch the daemon refused, or a second pass replacing it. */
export function clearReview(key: string): void {
  setStore(key, undefined!);
}
