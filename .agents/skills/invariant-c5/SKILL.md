---
name: invariant-c5
description: Runtime invariant checklist for harness reviewer (CHECKPOINTS C5). Use when the diff touches daemon core, persistence, PTY, or protocol handlers.
---

# Invariant C5 — runtime

- [ ] Lock order `inner -> registry`; no core lock across `.await` or blocking I/O.
- [ ] `pump_terminal` one critical section per PTY chunk; `FRAME` vs `IDLE_FRAME`.
- [ ] `emit_seq` once per **emitted** delta.
- [ ] Runtime-only fields have no SQLite column.
- [ ] Migrations append-only at end of `migrations.rs`.
- [ ] `SpawnSpec.env` complete; preserve `TERM`, `COLORTERM`, `FORGE_*`.
- [ ] Network commands: ack-on-start, coalesce flags, `GetSnapshot` no network.
- [ ] `GetWorkspaceDiff` / file ops / `GetUsageAnalytics`: local sync reads.

Example:

```bash
git diff -U0 crates/daemon/src/core.rs | grep '\.await'
```
