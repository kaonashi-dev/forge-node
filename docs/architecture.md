# Architecture

Forge is a native desktop app for running many terminals and coding agents
across Git repos and worktrees. This document is the system map and reflects
what is implemented; see the per-topic pages in [README.md](./README.md) for
details (domain, protocol, terminal, agents, worktrees, persistence) and
[`AGENTS.md`](../AGENTS.md) for invariants. Cost model before touching the
delta path, the Rune render path, the core lock, or anything that spawns a
process: [performance.md](./performance.md). ADR numbers: [decisions.md](./decisions.md).

## Principles

- **P1 Terminal-first.** `AgentSession = TerminalSession + AgentDescriptor`;
  a new CLI is supported with a descriptor. See [agents.md](./agents.md).
- **P2 Agent-agnostic.** No core logic does `if provider == claude`; every
  provider difference lives in `crates/agents`.
- **P3 The daemon owns execution.** GUI = presentation + interaction; daemon =
  processes, PTYs, sessions, Git, persistence. Closing the GUI kills nothing.
- **P4 The worktree is the isolated workspace.** The main checkout is a
  `Workspace` like any other, never a special case — including for the files a
  project shares between its checkouts, whose store lives in the repository's
  common git dir rather than in anybody's working copy. See
  [worktrees.md](./worktrees.md).
- **P5 Session graph from day one.** `parent_session_id` /
  `root_session_id` exist; the rail indents children under the parent.
  Cross-provider handoff, spawn-child and send-context are Forge-mediated — see
  [session-context.md](./session-context.md).
- **P6 Simplicity over speculative abstraction.** Git CLI, standard PTYs,
  descriptors, a Unix socket, one window.
- **P7 Native where it matters.** The GUI is the native Rune client
  (`apps/ui`); the daemon owns every PTY and the only VT engine.

## Two processes (ADR-003)

```
forge-ui      GUI (Rune)          — terminal and controls
forge-daemon  runtime             — PTYs, sessions, git, persistence
```

Closing the GUI never kills sessions; the daemon owns execution. They talk over
a Unix domain socket with length-prefixed MessagePack frames (ADR-004): `u32`
big-endian length + payload, 16 MiB max.

## Crates

```
domain           shared model: ids, project, workspace, session (+state machine),
                 agent descriptors/runtime types, context, and the terminal wire
                 types (Row/Cell/Cursor/Snapshot/Delta) shared by both sides
protocol         ClientMessage/DaemonMessage, framing, handshake, requests,
                 responses, events, errors — transport-agnostic
terminal-core    PtyBackend/PtyHandle (portable-pty), TerminalEngine trait,
                 AlacrittyEngine, DeltaBuilder
terminal-input   pure key/mouse/paste mapping shared without PTY/VT dependencies
editor-core      document, transactions, history, search and syntax; no I/O
editor-control   independent daemon–editor control framing and messages
editor-cli       standalone/integrated editor host      → binary forge-editor
agents           provider registry, verified detection, three built-ins
                 (claude, opencode, cursor)
git-service      git CLI wrapper (LC_ALL=C, 30s local timeout), repo + worktree ops
fs-service       workspace list/read/write/search; paths stay inside the checkout;
                 the go-to-definition provider trait and the grep heuristic
languages        language and language-server registry (languages.toml + user
                 overrides); root markers; no I/O but `exists`
syntax-index     tree-sitter tags/locals definition provider (grep-then-parse)
external-index   opt-in ctags and SCIP definition providers
lsp-client       blocking std-thread LSP client: framing, lifecycle, definition
persistence      SQLite (WAL + FK), migrations, repositories, orphan reconciliation
client           sync UDS client + passive CellGrid replica (no tokio)
daemon           the runtime: core dispatcher, server, registry, terminal loop,
                 shell env, services  → binary forge-daemon
forge-ctl        MCP-first client for runs, tasks, context and session dispatch
                 → binary forgectl. Depends only on client → protocol → domain.
test-support     FakePtyBackend, fake agents, temp git repos
```

`forge-ui` lives outside `crates/` — at `apps/ui` — because it is the GUI, not
a library: it owns a window, platform resources and its own Cargo workspace on
Rust 1.99.1 (Rune needs edition 2024). It sits on the same spine
(`forge-ui → client → {protocol, terminal-input} → domain`).

Dependency direction is one-way and enforced by `Cargo.toml`:
`forge-ui → {client, domain}`, `client → {protocol, terminal-input, domain}`,
`daemon → {agents, client, editor-control, fs-service, git-service, languages,
syntax-index, external-index, lsp-client, persistence, protocol, terminal-core,
terminal-input, domain}`, `fs-service → {git-service, languages}`,
`{syntax-index, external-index} → {fs-service, git-service, languages}`,
`editor-cli → {editor-core, editor-control}` and `test-support → {domain,
terminal-core}`. `forgectl` is `forge-ctl → client → protocol → domain`; its
binary and library never depend on `daemon` (only its integration tests boot
one).
Every other crate depends on `domain` alone or on nothing (`editor-core`,
`editor-control`, `languages`, `lsp-client`), there is no cycle, and the host depends on neither
`protocol` nor `terminal-core` directly. The GUI renders `domain::terminal` wire types, it does
not emulate. ADR numbers cited in code are indexed in [decisions.md](./decisions.md).

The Rune client runs blocking `client::Client` calls on a dedicated worker
thread. A flume command channel carries input, resize, migration and session
actions away from the UI thread; snapshots and connection updates return the
other way. Terminal deltas stay out of the shell's update path: the worker
applies every delta to the passive `Arc` grid, merges damage and wakes only the
terminal surface, so a frame of output never rebuilds the sidebar.

On startup the client connects to the per-user socket or starts the adjacent
`forge-daemon` (`--spawn-daemon`), adds the current directory when no project
exists, and creates or reuses a live shell. On disconnect it reconnects and
attaches a fresh authoritative snapshot. The workbench reads that answer inline
— `git diff`, a directory listing, a file read, a search — share the same
worker, because a subprocess or disk walk on the UI thread is a frozen window.

The clipboard, file dialogs, open URL/reveal, native menus and the updater are
host modules (`apps/ui/src/{menus,native_menus,updates}.rs`). The editor is not
in the GUI at all: it is `forge-editor`, supervised by the daemon. The cells
surface uses the terminal renderer and the DOM surface publishes bounded line
windows (`docs/editor.md`). External edits arrive through connection-scoped
directory watches (`WatchFiles`): `daemon::file_watch` coalesces native events
into `FileChanged`, the GUI re-reads only what it shows, and every read and
write still goes through `fs-service` (ADR-012).

The file tree reads real immediate children with `ListDirectory`, including the
root (`path: ""`), empty directories and dotfiles. Each directory retains its
last good children alongside request identity, generation, invalidation debt,
loading/error and partial status. Closed directories are read when opened;
file-change paths reconcile affected parents rather than scan the checkout.
`ListFiles` remains a separate bounded navigation index, loaded on demand by
the global file palette and terminal references and augmented by known lazy
directory entries. It never decides which directories exist in the explorer.

Workbench enqueue is fallible. Path mutations have operation IDs and an
explicit result event, separate from directory and file-read failures. Only
acknowledged moves retarget previews, parked views and recent paths; editor
sessions also follow authoritative session path metadata. A timeout or
disconnect retires the pending interaction as uncertain and schedules reads for
reconciliation, never automatically repeats the write. Watch acknowledgments
carry the exact generation of installed interests so stale ACKs cannot arm a
new subscription.

The file tree owns the pointer gesture and file-reference menu listener. A tree
drop reuses the confirmed-result rename API. A terminal drop sends a local
targeted paste with both session/terminal identities and the connection
generation, validated again before `encode_paste` writes to the existing PTY.
No file contents or filesystem access are involved in constructing the quoted
absolute reference.

### Client ownership

`apps/ui/src/` groups the shell (`ui.rs`, `chrome.rs`), the feature modules
(`projects.rs`, `workbench/`, `editor/`, `terminal/`, `settings/`, `history.rs`,
`processes.rs`), the host modules (`menus.rs`, `native_menus.rs`, `updates.rs`),
and the shared visual layer (`theme/`, `glyphs.rs`, `file_icons.rs`). The full
map is [ui.md](./ui.md). `client` stores are read-only replicas; a widget never
runs a subprocess, reads a workspace file or opens a socket on the UI thread.

## Authoritative terminal (ADR-011)

Exactly one VT engine runs, in the daemon. On attach it sends a full
`TerminalSnapshot`; then it sends `TerminalDelta`s carrying only damaged rows.
The client keeps a passive `CellGrid` of cells and applies row diffs.

- **Sequence:** each terminal has a per-emit `seq`. The client discards
  `seq ≤ last`, applies `seq == last+1`, and re-attaches on a gap (`> last+1`).
- **Coalescing:** the PTY loop drains available bytes, then emits a delta at
  most every ~8 ms (≤125/s). Read waits on `poll` until input or the next emit /
  synchronized-output deadline; it does not sleep after a successful read.
  Unwatched sessions still consume the child's output at full speed and simply
  skip emit.
- **One lock per batch:** feeding, routing and the activity bump all happen in a
  single core-lock acquisition (`Daemon::pump_terminal_batch`), not three.
- **Backpressure:** each client's outbound queue is bounded (256). When it fills
  the client is marked "behind" for that terminal and, once drained, gets a
  single fresh resync instead of a backlog — the PTY loop never blocks and daemon
  memory stays bounded.

## Sessions & the graph (ADR-010)

A session is a domain node (`Shell`, `Agent`, or `Editor`) with a state machine
(`Starting → Running → Exited/Orphaned/Failed`, restart back to `Starting`) and a
`parent_session_id` / `root_session_id`. The graph is logical, not the process
tree: children are re-parented to the grandparent on close, depth is capped at 8,
and cycles / cross-project parents are rejected. Kill signals the whole process
group (SIGHUP for shells, SIGTERM for agents, SIGKILL after the grace period), so
grandchildren never orphan.

Editor sessions hold an unsaved buffer and are unconditionally exempt from idle
stopping; see [editor.md](./editor.md) for their control channel and surfaces.

Every live session also carries a runtime-only `last_activity_at` — the last
moment its PTY produced output or received input — bumped at most once a second
by the PTY thread and never persisted. A sweeper thread reads it every 30 s and
applies the `[sessions]` idle policy (`crates/daemon/src/idle.rs`): a quiet
session is reported once per quiet spell, and, only if the user sets
`idle_stop_after_secs`, ended through the normal kill path so it stays
restartable. A second clock — age since creation — answers "open too long" and
never stops anything. A session waiting on the person shows how long it has
waited in its header.

A worktree Forge creates lives away from the repository, so it arrives without
the untracked files the project needs to run. Which ones follow it is a
per-project set of rules (`crates/daemon/src/shares`), and the split is
the one `idle.rs` uses: `plan.rs` decides as a pure function of the rules and
what is on disk, `apply.rs` performs the copy / CoW clone / symlink / command,
and `core.rs` owns which workspace and when. Provisioning runs on a worker and
reports with `SharesApplied` — a setup command is a subprocess, and the GUI
channel that would have carried the request also carries every keystroke.

## Persistence (ADR-009)

SQLite stores metadata only — organizational project groups, projects,
execution workspaces, sessions and their graph, context envelopes, provider
overrides, opaque app state. Never the terminal
stream (scrollback lives in bounded daemon memory). `terminal_id` is runtime-only
and never persisted.

A PTY never survives the daemon, so every session row a restart finds is dead.
What happens to it is `sessions.persist_history`: by default the daemon drops
the session rows on startup and the app opens on a clean tree; with the flag on
they are kept and reconciled to `Orphaned`, restartable through
`RestartSession`. Project groups, projects and workspaces survive either way.
`scripts/dev daemon` runs isolated (`FORGE_SOCKET` and `FORGE_HOME` under a
fresh temp dir), so a dev daemon never contends with the daily one and its
worktrees land in `$FORGE_DEV_DIR/worktrees`; one writer owns one set of paths.

## Concurrency

The daemon runs tokio (multi-thread). All domain state sits behind one
`Mutex<Inner>`; request handlers are synchronous and run via `spawn_blocking`
(git and PTY spawns block), so the lock is never held across `.await`. Each PTY
has a dedicated OS thread that feeds the engine and routes deltas. Lock order is
always `inner → registry`.
