# Architecture

Forge is a native desktop app for running many terminals and coding agents
across Git repos and worktrees. This document is the system map and reflects
what is implemented; see the per-topic pages in [README.md](./README.md) for
details (domain, protocol, terminal, agents, worktrees, persistence) and
[`AGENTS.md`](../AGENTS.md) for invariants. Cost model before touching the
delta path, the Tauri render path, the core lock, or anything that spawns a
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
- **P7 Native where it matters.** The GUI is Tauri + Solid (`apps/tauri`); the
  daemon owns every PTY and the only VT engine.

## Two processes (ADR-003)

```
forge-tauri   GUI (Tauri + Solid) — terminal and controls
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
agents           provider registry, verified detection, five built-ins
                 (claude, codex, opencode, cursor, grok)
git-service      git CLI wrapper (LC_ALL=C, 30s local timeout), repo + worktree ops
fs-service       workspace list/read/write/search; paths stay inside the checkout
harness-service  read/write of `<repo>/harness/` (one state machine per repository)
persistence      SQLite (WAL + FK), migrations, repositories, orphan reconciliation
client           sync UDS client + passive CellGrid replica (no tokio)
daemon           the runtime: core dispatcher, server, registry, terminal loop,
                 shell env, services  → binary forge-daemon
forge-tauri      Tauri host: runtime thread, workbench worker, cells encoder,
                 daemon locator                       → binary forge-tauri
test-support     FakePtyBackend, fake agents, temp git repos
```

`forge-tauri` lives outside `crates/` — at `apps/tauri/src-tauri` — because it
is a *host*, not a library: it owns a frontend build, a bundle configuration
and platform resources alongside its Rust. It is a workspace member like any
other and sits on the same spine (`forge-tauri → client → {protocol,
terminal-input} → domain`).

Dependency direction is one-way: `forge-tauri → client → {protocol,
terminal-input} → domain` and
`daemon → {agents, git-service, fs-service, harness-service, persistence,
terminal-core} → domain`. The GUI renders `domain::terminal` wire types, it does
not emulate. ADR numbers cited in code are indexed in [decisions.md](./decisions.md).

The Tauri host runs blocking `client::Client` calls on a dedicated runtime thread. A
flume command channel carries input, resize and session actions away from the
render thread; store snapshots and connection updates return the other way —
Tauri emits `runtime:state` and `runtime:cells` as
separate events so a frame of terminal output never drags the session tree
through `serde_json` with it. On startup the bridge connects to the per-user
socket or starts the adjacent `forge-daemon`, adds the current directory when
no project exists, and creates or reuses a live shell. On disconnect it
reconnects and attaches a fresh authoritative snapshot.

The Tauri host also has a *workbench worker* sharing the same `Client`. The command
channel carries keystrokes, and the local reads that answer inline — `git diff`, a
file tree, a search, the harness's own files — are seconds of subprocess, so
running them there would freeze typing.

The file surface splits presentation from IO the same way. The windowed explorer
lives in `apps/tauri/packages/file-workbench`, which has no Solid, Tauri or
filesystem dependency; `apps/tauri` adapts that package to the daemon's reads
and Forge's theme. Editing is not in that package and is not in the GUI at all:
it is `forge-editor`, supervised by the daemon: the cells surface uses the
terminal renderer and the headless DOM surface publishes bounded line windows
(`docs/editor.md`). External edits arrive through
connection-scoped directory watches (`WatchFiles`): `daemon::file_watch`
coalesces native events into `FileChanged`, the GUI re-reads only what it shows,
and every read and write still goes through `fs-service` (ADR-012).

The explorer reads real immediate children with `ListDirectory`, including the
root (`path: ""`), empty directories and dotfiles. Each directory retains its
last good children alongside request identity, generation, invalidation debt,
loading/error and partial status. Closed directories are read when opened;
file-change paths reconcile affected parents rather than scan the checkout.
`ListFiles` remains a separate bounded navigation index, loaded on demand by
the global file palette and terminal references and augmented by known lazy
directory entries. It never decides which directories exist in the explorer.

Workbench enqueue is fallible. Path mutations have frontend operation IDs and
an explicit result event, separate from directory and file-read failures.
Only acknowledged moves retarget previews, parked views and recent paths;
editor sessions also follow authoritative session path metadata. A timeout or
disconnect retires the pending interaction as uncertain and schedules reads
for reconciliation, never automatically repeats the write. Watch acknowledgments
carry the exact generation of installed interests so stale ACKs cannot arm a
new subscription.

`workbench/fileDrag.ts` owns the explorer's pointer gesture and file-reference
menu listener. The portable explorer only exposes row metadata and expansion;
`TerminalPane` registers its live session/terminal identity. Tree drops reuse
the confirmed-result rename API. Terminal drops send a host-local targeted paste
with both identities and the connection generation, validated again before
`encode_paste` writes to the existing PTY. No file contents or filesystem access
are involved in constructing the quoted absolute reference.

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
never stops anything. The sidebar shows the same idleness as a dim `45m` on the
session row once it passes ten minutes.

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
`scripts/dev daemon` runs isolated (`FORGE_SOCKET`/`FORGE_DATA_DIR`/
`FORGE_CONFIG_DIR` under a fresh temp dir), so a dev daemon never contends
with the daily one; one writer owns one set of paths.

## Concurrency

The daemon runs tokio (multi-thread). All domain state sits behind one
`Mutex<Inner>`; request handlers are synchronous and run via `spawn_blocking`
(git and PTY spawns block), so the lock is never held across `.await`. Each PTY
has a dedicated OS thread that feeds the engine and routes deltas. Lock order is
always `inner → registry`.
