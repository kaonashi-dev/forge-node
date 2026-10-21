# Decision index

Numbers of the form `ADR-00N` appear in code and docs. The original decision
records were never published; this table is the current meaning, as
implemented. Do not restore them from `plan/`.

| Id | Decision | Where it lives |
|----|----------|----------------|
| ADR-002 | License/advisory policy for distribution (`cargo deny`) | `deny.toml`. Not part of `scripts/dev check` or CI. |
| ADR-003 | Two processes: `forge-tauri` (GUI) and `forge-daemon` (runtime) | [`architecture.md`](./architecture.md) |
| ADR-004 | Unix domain socket, length-prefixed MessagePack, 16 MiB max frame | `crates/protocol/src/framing.rs` |
| ADR-005 | The daemon owns every PTY | `crates/terminal-core` |
| ADR-006 | An agent is a terminal session running a known CLI | [`agents.md`](./agents.md) |
| ADR-007 | Declarative provider registry: descriptors, not integrations | `crates/agents` |
| ADR-008 | Git CLI first: `LC_ALL=C`, no prompts, 30 s local timeout, never in a user PTY, `git status` throttled to one run per workspace per 2 s. Network commands use `run_git_network` (default 120 s) | `crates/git-service/src/command.rs`, [`worktrees.md`](./worktrees.md) |
| ADR-009 | SQLite stores metadata only — never the terminal stream, never a runtime `terminal_id` | [`persistence.md`](./persistence.md) |
| ADR-010 | Sessions form a logical graph (parent/child, depth ≤ 8), not the process tree | [`domain.md`](./domain.md) |
| ADR-011 | One VT engine, in the daemon. The client is a passive cell replica | [`terminal.md`](./terminal.md), `crates/client` |
| ADR-012 | The GUI never does `std::fs` on a workspace. Files, search, and writes go through the daemon | `crates/fs-service`, [`protocol.md`](./protocol.md) |
| ADR-013 | Go to definition is an ordered chain of daemon-side providers — opt-in language servers, a SCIP index, the bundled tree-sitter index, opt-in ctags, then the grep heuristic — merged by confidence. Only a resolver may make the editor jump without asking. Language servers are daemon-owned, lazy, bounded and never configured by a repository. A place outside the checkout travels as an opaque id and opens read-only | `crates/fs-service/src/definition.rs`, `crates/languages`, `crates/syntax-index`, `crates/external-index`, `crates/lsp-client`, `crates/daemon/src/{definitions,lsp}.rs`, [`editor.md`](./editor.md) |

There is no ADR-001 in the tree.
