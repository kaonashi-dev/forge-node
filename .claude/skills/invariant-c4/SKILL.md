---
name: invariant-c4
description: Crate boundary checklist for harness reviewer (CHECKPOINTS C4). Use when the diff touches crate dependencies or cross-crate imports.
---

# Invariant C4 — crate boundaries

Grep the diff; do not rely on memory.

- [ ] `terminal-core` only in daemon; `client`/`apps/tauri` passive snapshot replica.
- [ ] Tauri host wire types via `client` re-exports; frontend through runtime bridge only; no direct `protocol` dependency in `apps/tauri/src-tauri/Cargo.toml`.
- [ ] No `provider_id` branching outside `crates/agents`.
- [ ] Git via `git_service::run_git` / `run_git_network`; host API only in `github.rs`.
- [ ] Tauri colours/metrics from `apps/tauri/src/theme/tokens.ts`; repeated UI from `apps/tauri/src/ui`.
- [ ] `domain` serializable; `protocol` framing; `#[non_exhaustive]` wildcards.

Example greps:

```bash
git diff -U0 | grep -n 'provider_id'
git diff -- apps/tauri/src-tauri/Cargo.toml | grep protocol
```
