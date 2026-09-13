# Context

Forge Node is a two-process desktop workbench. `forge-daemon` owns PTYs, the
only VT engine, sessions, Git, and SQLite. `forge-tauri` (Tauri 2 + **Solid**,
not React) is a passive replica of a cell grid. Closing the window kills
nothing.

## Vocabulary

The glossary lives in [`docs/domain.md`](docs/domain.md): Project group,
Project, Workspace, Session, Terminal, Agent provider, session graph, context
envelope, managed worktree.

## Map

| Need | Read |
|------|------|
| System map, crate graph, two processes | [`docs/architecture.md`](docs/architecture.md) |
| Invariants agents must preserve | [`AGENTS.md`](AGENTS.md) |
| ADR numbers cited in code (`ADR-003` …) | [`docs/decisions.md`](docs/decisions.md) |
| Cost rungs (per cell / delta / frame / request) | [`docs/performance.md`](docs/performance.md) |
| Commands and "where to change X" | [`docs/development.md`](docs/development.md) |
| GUI layout, tokens, keymap | [`docs/ui.md`](docs/ui.md) |
| Repeatable architecture/quality review | `/forge-clean-code` (`.grok/skills/forge-clean-code`) |
| Wire protocol | [`docs/protocol.md`](docs/protocol.md) |
| Providers | [`docs/agents.md`](docs/agents.md) |
| Doc index | [`docs/README.md`](docs/README.md) |

## Not sources of truth

- Local `plan/` working notes (gitignored). They can contradict the code.
- `apps/tauri/PARITY.md` and `PROGRESS.md` — historical phase registers.
