// Keeps the rail's branch current after a `git checkout` typed into a terminal.
//
// `Workspace.branch` is only re-read from places that already know git moved, and
// `daemon::file_watch` is non-recursive and watches only the focused checkout, so
// neither sees a bare `checkout`. This re-reads status when the active terminal
// goes quiet (sampled, not debounced per frame) and when the window regains
// focus, never at a fixed rate: a `git status` on a large checkout is real
// subprocess time.

import { cellsChannel } from "../../runtime/bus";
import { refreshWorkspaceStatus } from "../sessions/commands";
import { forgeStore } from "../../state/forgeStore";
import { connectionStore } from "../../state/connection";

/** Quiet time after the last frame before a burst counts as finished. */
export const QUIET_MS = 900;

/** How often the quiet is checked for. Never the rate anything is read at. */
export const TICK_MS = 500;

/**
 * The soonest two reads may follow each other.
 *
 * A shell alternating between output and quiet — a test watcher, a dev server
 * printing every save — would otherwise ask once per burst forever.
 */
export const FLOOR_MS = 3_000;

export type SyncClock = {
  now: number;
  /** When the last terminal frame arrived, or 0 if none has. */
  lastFrameAt: number;
  /** When a read was last asked for. */
  lastSyncAt: number;
  /** Whether a frame has arrived since the last read. */
  dirty: boolean;
};

/**
 * Whether the terminal has been quiet long enough to be worth a read.
 *
 * Pure and exported so the three rules — something happened, it has stopped
 * happening, and we did not just ask — are testable without a clock or a PTY.
 */
export function shouldSync(clock: SyncClock): boolean {
  if (!clock.dirty) return false;
  if (clock.now - clock.lastFrameAt < QUIET_MS) return false;
  return clock.now - clock.lastSyncAt >= FLOOR_MS;
}

/**
 * Re-read the active checkout's status when its terminal goes quiet, and when
 * the window is focused again.
 *
 * Returns the teardown, so the caller owns the listeners.
 */
export function startGitSync(): () => void {
  let lastFrameAt = 0;
  let lastSyncAt = 0;
  let dirty = false;

  /*
   * Two assignments on the frame rung and nothing else.
   *
   * `bus.ts` exists because a store write on this path deep-proxies a payload
   * that arrives 62 times a second; a timer reset per frame would be the same
   * mistake in miniature, which is why the quiet is *sampled* below instead of
   * debounced here.
   */
  const unsubscribe = cellsChannel.subscribe(() => {
    lastFrameAt = Date.now();
    dirty = true;
  });

  /** The checkout the visible terminal belongs to. */
  function activeWorkspace(): string | null {
    const session = forgeStore.sessions.find((item) => item.id === connectionStore.activeSession);
    return session?.workspace_id ?? null;
  }

  function sync(): void {
    const workspace = activeWorkspace();
    if (!workspace) return;
    lastSyncAt = Date.now();
    dirty = false;
    // The answer arrives as `WorkspaceUpdated`, which the rail already
    // reconciles; a failed read leaves the previous branch on screen, which is
    // what was there anyway.
    void refreshWorkspaceStatus(workspace).catch(() => undefined);
  }

  const timer = setInterval(() => {
    if (shouldSync({ now: Date.now(), lastFrameAt, lastSyncAt, dirty })) sync();
  }, TICK_MS);

  /*
   * Coming back to the window is the other moment: git may have moved in a
   * terminal outside this app, or in a worktree an agent owns. Unconditional —
   * a person who just alt-tabbed back is asking to see the current state, and
   * there is no burst to wait out.
   */
  const onFocus = (): void => sync();
  window.addEventListener("focus", onFocus);

  return () => {
    unsubscribe();
    clearInterval(timer);
    window.removeEventListener("focus", onFocus);
  };
}
