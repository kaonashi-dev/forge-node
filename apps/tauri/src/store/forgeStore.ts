import { createStore, reconcile } from "solid-js/store";
import type { ShellSnapshot } from "../runtime/types";

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
 * and in a stable order.
 */
export function applyShellSnapshot(snapshot: ShellSnapshot): void {
  setForgeStore(reconcile(snapshot, { key: "id" }));
}
