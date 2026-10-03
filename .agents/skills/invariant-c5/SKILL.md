---
name: invariant-c5
description: Runtime invariant checklist (C5). Use when the diff touches daemon core, persistence, PTY, or protocol handlers.
---

# Invariant C5 — runtime

- [ ] Lock order `inner -> registry`; no core lock across `.await` or blocking I/O.
- [ ] `pump_terminal_batch` one critical section per processed batch; deadline-driven `poll`, no post-read sleep; `FRAME` limits attached emits, not reads.
- [ ] `emit_seq` once per **emitted** delta.
- [ ] Runtime-only fields (`terminal_id`, `last_activity_at`, `Workspace::status`, `PullRequest`, diffs, rebase state) have no SQLite column; `Session::base_commit` is the one column, resolved between lock sections, never inside one. Session activity is one of them. `forgectl hook` does not write SQLite, and only an explicit report settles an attempt.
- [ ] Migrations append-only at end of `migrations.rs`.
- [ ] `SpawnSpec.env` complete; preserve `TERM`, `COLORTERM`, `FORGE_*`.
- [ ] Network commands: ack-on-start, coalesce flags, `GetSnapshot` no network.
- [ ] Every coalescing flag (`fetching`, `pr_opening`, `pr_refreshing`, `provisioning`, `drafting`, `detecting`) is released by a `Drop` guard.
- [ ] `pull_requests`, `external_agents` and `usage_stats` caches keep their own `Mutex`, never the core one.
- [ ] The socket lock and the data-dir lock are both held before SQLite opens.
- [ ] `SessionKind::Editor` is exempt from idle stop by kind, unconditionally.
- [ ] `PRAGMA synchronous = NORMAL` stays: deliberate under WAL.
- [ ] `GetWorkspaceDiff` / file ops / `GetUsageAnalytics`: local sync reads.
- [ ] "Resolve with AI" launches an ordinary agent session. Its rebase prompt permits `--continue` only after staging reviewed resolutions and running relevant checks; ambiguous intent or failed verification stops for a human decision. Merge and cherry-pick prompts leave paths unstaged and continuation to the person.

Example:

```bash
git diff -U0 crates/daemon/src/core.rs | grep '\.await'
```
