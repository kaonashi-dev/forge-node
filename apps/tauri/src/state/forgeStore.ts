import { batch, createSignal } from "solid-js";
import { createStore, reconcile, unwrap } from "solid-js/store";
import type { ShellSnapshot } from "../contracts/runtime";
import { agentVisible } from "./agentVisibility";
import { setLoading } from "./loading";

export const emptySnapshot = (): ShellSnapshot => ({
  project_groups: [],
  projects: [],
  workspaces: [],
  sessions: [],
  providers: [],
  usage: [],
  external_agents: [],
  pull_requests: {
    pull_requests: [],
    viewers: [],
    sources: [],
    failures: [],
    error: null,
    refreshed_at: null,
  },
  launchables: [],
  worktree_shares: [],
  project_actions: [],
  worktree_ignores: [],
  agent_profiles: [],
  session_attention: {},
  app_state: {},
  live_sessions: 0,
  installed_agents: 0,
  provider_count: 0,
});

export const [forgeStore, setForgeStore] = createStore<ShellSnapshot>(emptySnapshot());

/**
 * Take a fresh snapshot, keeping the identity of every row that did not change.
 *
 * `setForgeStore(snapshot)` writes the arrays the IPC just parsed, so every one
 * of them is a new reference — and `<For>` maps by identity. The rail, the tab
 * strip and every panel list were torn down and rebuilt on each snapshot,
 * however little of it had moved: a background agent's unread badge rebuilt the
 * whole shell. `reconcile` diffs by `id` and writes only the leaves that
 * actually differ, so a session whose title changed repaints that row and
 * nothing else, and a snapshot that changed nothing notifies nobody.
 *
 * Rows without an `id` — providers, launchables, usage — fall back to a
 * positional merge, which is the right answer for lists the daemon sends whole
 * and in a stable order. Discovered runs and pull requests are neither: one new
 * transcript or a re-sorted pull request would shift every later row, so they
 * are keyed by `session_id` and `url` instead.
 */
const [snapshotLanded, setSnapshotLanded] = createSignal(false);

/**
 * Whether a daemon snapshot has been applied. Unlike a non-empty `app_state`,
 * this is true for a fresh daemon that has stored nothing yet.
 */
export { snapshotLanded };

export function applyShellSnapshot(snapshot: ShellSnapshot): void {
  const before = forgeStore.pull_requests;
  const answered =
    snapshot.pull_requests.refreshed_at !== before.refreshed_at ||
    snapshot.pull_requests.error !== before.error;
  batch(() => {
    setForgeStore(
      reconcile(
        {
          ...snapshot,
          launchables: snapshot.launchables.filter(
            (item) => item.kind === "shell" || agentVisible(snapshot.app_state, item.key),
          ),
          // The store's own values, which reconcile skips; they are keyed below.
          external_agents: unwrap(forgeStore.external_agents),
          pull_requests: unwrap(forgeStore.pull_requests),
        },
        { key: "id" },
      ),
    );
    setForgeStore("external_agents", reconcile(snapshot.external_agents, { key: "session_id" }));
    setForgeStore("pull_requests", reconcile(snapshot.pull_requests, { key: "url" }));
    setSnapshotLanded(true);
  });
  // A refresh acks when it starts and is answered by the next snapshot.
  if (answered) setLoading("pull_requests", false);
}
