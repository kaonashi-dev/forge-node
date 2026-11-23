# Decision index

Numbers of the form `ADR-00N` appear in code and docs. The original decision
records were never published; this table is the current meaning, as
implemented. Do not restore them from `plan/`.

| Id | Decision | Where it lives |
|----|----------|----------------|
| ADR-002 | License/advisory policy for distribution (`cargo deny`) | `deny.toml`. Not part of `scripts/dev check` or CI. |
| ADR-003 | Two processes: `forge-ui` (GUI) and `forge-daemon` (runtime) | [`architecture.md`](./architecture.md) |
| ADR-004 | Unix domain socket, length-prefixed MessagePack, 16 MiB max frame | `crates/protocol/src/framing.rs` |
| ADR-005 | The daemon owns every PTY | `crates/terminal-core` |
| ADR-006 | An agent is a terminal session running a known CLI | [`agents.md`](./agents.md) |
| ADR-007 | Declarative provider registry: descriptors, not integrations | `crates/agents` |
| ADR-008 | Git CLI first: `LC_ALL=C`, no prompts, 30 s local timeout, never in a user PTY, `git status` throttled to one run per workspace per 2 s. Network commands use `run_git_network` (default 120 s) | `crates/git-service/src/command.rs`, [`worktrees.md`](./worktrees.md) |
| ADR-009 | SQLite stores metadata only — never the terminal stream, never a runtime `terminal_id` | [`persistence.md`](./persistence.md) |
| ADR-010 | Sessions form a logical graph (parent/child, depth ≤ 8), not the process tree | [`domain.md`](./domain.md) |
| ADR-011 | One VT engine, in the daemon. The client is a passive cell replica | [`terminal.md`](./terminal.md), `crates/client` |
| ADR-012 | The GUI never does `std::fs` on a workspace. Files, search, and writes go through the daemon | `crates/fs-service`, [`protocol.md`](./protocol.md) |
| ADR-013 | Go to definition is an ordered chain of daemon-side providers — opt-in language servers, a SCIP index, the bundled tree-sitter index, opt-in ctags, then the grep heuristic — merged by confidence. Only a single answer that an authoritative provider parsed or resolved may make the editor jump without asking; a heuristic match is always a list. Language servers are daemon-owned, lazy, bounded and never configured by a repository. A place outside the checkout travels as an opaque id and opens read-only | `crates/fs-service/src/definition.rs`, `crates/languages`, `crates/syntax-index`, `crates/external-index`, `crates/lsp-client`, `crates/daemon/src/{definitions,lsp}.rs`, [`editor.md`](./editor.md) |
| ADR-020 | **Forge Review v2.** A project's codebase graph lives in SQLite (`codebase_graph_nodes`/`_edges`, keyed by project and shared by its worktrees) and is mirrored in a gitignored `.forge/review/graph/cache.bin` for `forgectl` without a daemon. It is invalidated by `HEAD`'s oid and rebuilt incrementally; no graph database | `crates/codebase-graph`, `crates/daemon/src/graph.rs`, [`review.md`](./review.md) |
| ADR-021 | **Forge Review v2.** Parallel reviewers are shared-nothing: each one is an ordinary read-only session with its own transcript, one shared pack and its own slice, validated alone and consolidated after; one reviewer is the v1 reviewer | `crates/review-hypotheses`, `crates/daemon/src/review.rs` |
| ADR-022 | **Forge Review v2.** The knowledge base is SQLite per project (`kb_entries`), redacted on write, never in the checkout: `architecture.md` regenerated whole, patterns appended and pruned after 90 days, learned rules and false positives derived from feedback at read time | `crates/knowledge-base`, `crates/daemon/src/kb.rs` |
| ADR-023 | **Forge Review v2.** Sandbox checks run the repository's `checks.toml` commands as written under macOS `sandbox-exec` (no network, writes only to the run, cache and temp, cleared environment, process-group kill). No Docker or Linux runner: elsewhere it says so | `crates/runtime-sandbox`, `crates/daemon/src/sandbox.rs` |
| ADR-024 | **Forge Review v2.** `forge.review/v2` is accepted beside v1; a v1 answer gets no hypothesis, v2's `pr_confidence` and `diagrams` are ignored (the score is v1's, diagrams are Forge's own deterministic ones) | `crates/review/src/output.rs`, `crates/diagram-gen` |
| ADR-025 | **Forge Review v2.** A knowledge base is never shared between projects; cross-repo context reads only other registered projects' graph signatures, never their bodies or knowledge base | `crates/daemon/src/{kb,graph}.rs` |

There is no ADR-001 in the tree.
