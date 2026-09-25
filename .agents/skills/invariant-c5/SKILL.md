---
name: invariant-c5
description: Runtime invariant checklist (C5). Use when the diff touches daemon core, persistence, PTY, or protocol handlers.
---

# Invariant C5 — runtime

- [ ] Lock order `inner -> registry`; no core lock across `.await` or blocking I/O.
- [ ] `pump_terminal_batch` one critical section per processed batch; deadline-driven `poll`, no post-read sleep; `FRAME` limits attached emits, not reads.
- [ ] `emit_seq` once per **emitted** delta.
- [ ] Runtime-only fields have no SQLite column. Session activity is one of them. `forgectl hook` does not write SQLite, and only an explicit report settles an attempt.
- [ ] Migrations append-only at end of `migrations.rs`.
- [ ] `SpawnSpec.env` complete; preserve `TERM`, `COLORTERM`, `FORGE_*`.
- [ ] Network commands: ack-on-start, coalesce flags, `GetSnapshot` no network.
- [ ] `GetWorkspaceDiff` / file ops / `GetUsageAnalytics`: local sync reads.

Example:

```bash
git diff -U0 crates/daemon/src/core.rs | grep '\.await'
```
