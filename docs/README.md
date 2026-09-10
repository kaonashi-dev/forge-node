# Forge documentation

Forge (ForgeNode) is a native desktop app for running many terminals and
coding agents across Git repositories and worktrees. These pages describe
**what is implemented**. Working plans and historical notes live in a local
`plan/` directory (gitignored via `.git/info/exclude`); they are not part of
this published tree. [`AGENTS.md`](../AGENTS.md) is the invariant list that
must match the code.

| Page | Read it when you need… |
|------|------------------------|
| [architecture.md](./architecture.md) | the two-process model, the crate map and dependency direction |
| [domain.md](./domain.md) | the glossary, entities, session state machine and graph rules, wire types |
| [protocol.md](./protocol.md) | every request/response/event, error codes, the `client` crate |
| [terminal.md](./terminal.md) | PTY → engine → delta pipeline, sequence/resync, backpressure, kill semantics, shell environment |
| [agents.md](./agents.md) | built-in providers, detection algorithm, launch spec |
| [worktrees.md](./worktrees.md) | project discovery, managed worktree placement/slugs, create/remove safety rules |
| [persistence.md](./persistence.md) | SQLite schema, migration rules, startup reconciliation |
| [ui.md](./ui.md) | the window layout, theme tokens, terminal rendering and the keyboard map |
| [performance.md](./performance.md) | the cost model — what runs per cell, per delta, per frame; the memory, CPU, process and lock rules and the defects behind them |
| [development.md](./development.md) | prerequisites, commands, running the daemon, testing notes |
| [commits.md](./commits.md) | commit subject/body and pull-request title/description |
| [harness.md](./harness.md) | subagent orchestrator cycle: `/feature` → spec gate → `/feature-go`, plus `scripts/harness` |
| [config.example.toml](./config.example.toml) | every `config.toml` key with its default |

## Ten-second mental model

```
forge-tauri (GUI)  ──UDS + MessagePack──►  forge-daemon
  Tauri + SolidJS                           owns PTYs, the only VT engine,
  renders cells from a replica              sessions, git, SQLite
```

- **Terminal-first:** an agent is a CLI (`claude`, `codex`, `opencode`,
  `cursor-agent`) running in a PTY. Providers are descriptors, not
  integrations.
- **The daemon owns execution:** closing the GUI never kills a session; the
  daemon is a per-user singleton.
- **Worktree = isolated workspace:** the main checkout is just another
  workspace; worktrees Forge creates are `managed_by_app` and the only ones it
  will ever delete. Branches are never deleted.
- **Sessions form a graph** (parent/child, depth ≤ 8) that is logical, not the
  process tree.
- **Single source of truth for the grid:** the daemon sends a snapshot on
  attach and damaged-row deltas afterwards; the GUI never emulates.

## Current status

Backend complete and tested (`scripts/dev check` is the gate). The Phase 0 GUI
is complete for the requested local macOS scope: it connects or starts the
daemon, renders and drives a live PTY, launches the four installed agent TUIs,
reattaches after disconnects, and reports key-to-render latency. Reference
smokes cover real agents, `vim`, `htop`, reconnect and the ≤33 ms byte-to-grid
budget. Wayland/X11 validation is deliberately deferred and remains required
before claiming Linux support. `forge-daemon dump`/`stats` are not wired. See
[`../AGENTS.md`](../AGENTS.md) for the invariants contributors must preserve.
