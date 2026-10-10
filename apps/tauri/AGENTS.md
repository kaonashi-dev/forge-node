# Tauri frontend

Start with [docs/frontend-architecture.md](../../docs/frontend-architecture.md):
folder ownership, the dependency edges `bun run boundaries` enforces, the
state/lifecycle owners, and where to change a behavior. Root
[AGENTS.md](../../AGENTS.md) carries the invariants that bind this app.

Gates: `bun run verify` here; `scripts/dev check` from the repository root when
Rust changed. Bun tracks `latest` with a 1.4.3 floor for `bun check`.
