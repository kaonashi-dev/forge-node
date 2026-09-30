---
name: invariant-c4
description: Crate boundary checklist (C4). Use when the diff touches crate dependencies or cross-crate imports.
---

# Invariant C4 — crate boundaries

Grep the diff; do not rely on memory.

- [ ] `terminal-core` only in daemon; `client`/`apps/tauri` passive snapshot replica.
- [ ] Tauri host wire types via `client` re-exports; frontend through runtime bridge only; no direct `protocol` dependency in `apps/tauri/src-tauri/Cargo.toml`.
- [ ] No `provider_id` branching outside `crates/agents`.
- [ ] Git via `git_service::run_git` / `run_git_network`; host API only in `github.rs`.
- [ ] A command that opens a socket goes through `run_git_network`, never `run_git`; `push` only from `CreatePullRequest`, never a sweeper.
- [ ] The GUI never does `std::fs` on a workspace (ADR-012); unknown `SearchKind` is `InvalidRequest`, never `Name`; `WriteFile` needs the `revision` from the last read.
- [ ] `client::Client` stays blocking `std` with a reader thread, no Tokio; `Client::request` is bridged off the UI thread; the reader never blocks on the bounded event queue.
- [ ] New Cargo edges match `docs/architecture.md`; an undocumented edge is a decision, not a detail.
- [ ] Tauri colours/metrics from `apps/tauri/src/theme/tokens.ts`; repeated UI from `apps/tauri/src/ui`.
- [ ] `domain` serializable; `protocol` framing; `#[non_exhaustive]` wildcards.
- [ ] `forgectl` (`forge-ctl`) depends only on `client → protocol → domain`, never on `daemon`.

Example greps:

```bash
git diff -U0 | grep -n 'provider_id'
git diff -- apps/tauri/src-tauri/Cargo.toml | grep protocol
```
