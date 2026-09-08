# Implementation plan — Desktop Agent Terminal (ForgeNode)

**Status:** architecture and execution proposal — revision 2
**Date:** 22 August 2026
**Target platforms:** macOS (arm64 first) + Linux (Wayland and X11)
**Chosen stack:** Rust + Tauri 2 + Solid/Vite + our own daemon
**Initial providers:** Claude Code, Codex CLI, OpenCode, Cursor CLI
**Main experience reference:** Orca
**Product name:** pending. In this document `{APP}` is a placeholder for the final name (binaries `{app}` and `{app}-daemon`, directories `{app}/`).

---

## 0. Changes from version 1

This revision does not change the plan's direction. It removes ambiguities that prevented starting the implementation without taking implicit decisions, and fixes internal contradictions.

| # | Change | Reason |
|---|--------|--------|
| 1 | Phases reordered: Terminal runtime before Projects | §18 contradicted §28 and §36 |
| 2 | New **ADR-011**: the daemon is the single source of truth for the grid; the GUI receives a snapshot + row diffs | §10.5 left open whether the GUI replicates bytes or cells. It is the most expensive technical decision to change later |
| 3 | Unified `SessionState` (`Starting/Running/Exited/Failed/Orphaned`) | There were three different variants in §3.2, §7.3 and §15.3 |
| 4 | `AgentDescriptor` and satellite types defined once | Three incompatible definitions; `SpawnSpec`, `TerminalSnapshot`, `ResolvedEnvironment`, `AgentCapabilities`, `PtySize` were undefined |
| 5 | Protocol completed: `CreateChildSession`, `RestartSession`, `StopDaemon`, `SetProviderExecutable`, `FetchScrollback`, `ListWorkspaces`; `FocusSession` removed; error model; frame limits | Requests the phases required did not exist; `FocusSession` violated ADR-003 |
| 6 | Socket in `$TMPDIR` on macOS with length validation | `~/Library/Caches` is purgeable and macOS limits a UDS path to 104 bytes |
| 7 | Provider detection verified with `--version` + a timeout | `agent` (Cursor) is a generic name that can collide |
| 8 | Semantics defined: Close vs Kill, parents with children, removing a project, worktree slug, title, grace period, backpressure | They were unspecified |
| 9 | `ShellEnvironmentService` with a sentinel, a timeout and a fallback | `$SHELL -l -c env` without a timeout or multiline handling is fragile |
| 10 | Closed decisions: X11 required, 10k scrollback by default, numeric budgets, `config.toml`, bundled monospace font | They were "to be decided" or "if applicable" |
| 11 | New section 39: glossary and a table of resolved ambiguities | A quick reference for the team |

---

## 1. Executive summary

The product will be a native desktop application for **working with multiple terminals and multiple coding agents over one or several Git repositories**, with explicit support for worktrees and with an architecture ready from the start for a session to originate other sessions.

In the MVP, the application will not try to be an IDE nor a proprietary structured interface for each AI provider. Its fundamental unit is a **persistent terminal session** associated with a workspace (the main checkout or a worktree). An agent is, initially, a specialisation of that session: a PTY running a known CLI (`claude`, `codex`, `opencode`, `agent`/`cursor-agent`).

Technology decision:

- **Rust** for the daemon, protocol, and Tauri host.
- **Tauri 2 + Solid/Vite** for the desktop GUI (WebView chrome + Canvas terminal).
- **Our own daemon**, owning PTYs, processes, sessions and operational state, independent of the window's lifecycle.
- **Unix Domain Socket + MessagePack** for GUI ↔ daemon.
- **Git CLI** for Git and worktree operations.
- **SQLite** for projects, workspaces, sessions, hierarchy and preferences.
- **`alacritty_terminal`** as the terminal engine behind a `TerminalEngine` trait (confirmed or replaced in Phase 0).

The architecture will have a **session graph** from the beginning, not a flat list:

```text
Orchestrator
├── Planner
├── Researcher
├── Executor
│   ├── Test Runner
│   └── Fixer
└── Reviewer
```

The parent/child relation is **logical and persistent**, not a dependency of the OS process tree. Each child can have its own PTY, agent, worktree and lifecycle.

### MVP definition

The MVP is complete when milestone **M6** (§33) is reached and scenarios A–F (§19) pass. Concretely, the MVP includes:

1. Opening the application; the daemon starts automatically.
2. Adding a project by selecting a local directory.
3. Detecting whether it is a Git repository and showing the branch and basic status.
4. Creating a normal terminal or an agent session.
5. Choosing between Claude, Codex, OpenCode and Cursor.
6. Running multiple sessions in parallel with tabs and splits.
7. Creating, listing and deleting managed Git worktrees, and launching sessions in them.
8. Showing sessions grouped by project/workspace in an Orca-inspired UI.
9. Closing and reopening the GUI without killing the sessions.
10. Restoring a terminal's view when reconnecting to the daemon without losing output.
11. Manually creating child sessions with a role, and having the nesting persist.

Point 11 was implicit in v1 (Phase 7, Scenario E, M6) but not in the scope list. It is now explicit: **the manual session graph is part of the MVP; automatic orchestration is not.**

---

# 2. Product context

## 2.1 Problem

Coding agents are capable, but the real flow is still fragmented across terminals, branches, worktrees and independent sessions:

```text
main repo
├── terminal: claude
├── terminal: codex
├── terminal: npm dev
├── worktree feature-a
│   └── terminal: opencode
└── worktree feature-b
    ├── terminal: cursor
    └── terminal: tests
```

The initial value proposition:

> A native desktop for organising, launching, watching and maintaining agent and terminal sessions isolated per project/worktree.

We do not need to own the model, the agent's protocol nor its conversation interface to deliver value.

## 2.2 Principles

**P1. Terminal-first.** `AgentSession = TerminalSession + AgentDescriptor`. A new CLI is supported with a descriptor.

**P2. Agent-agnostic.** No central logic does `if provider == Claude`. Providers are resolved through descriptors/adapters in a registry. Every difference between providers lives in the `agents` crate.

**P3. The daemon owns execution.** `GUI = presentation + interaction`. `Daemon = processes + PTYs + sessions + Git + operational persistence`. Closing the GUI does not kill agents.

**P4. The worktree is the isolated workspace.** A session works in a `Workspace` (main checkout, managed worktree, or in the future a remote workspace). The main checkout is a Workspace like any other, not a special case.

**P5. Session graph from day one.** Even though the MVP shows almost a list, the model is a tree with `parent_session_id` and `root_session_id`.

**P6. Simplicity before speculative abstraction.** Git CLI before reimplementing Git. A standard PTY before a per-provider protocol. Descriptors before a plugin SDK. A Unix socket before HTTP. One window before multi-window.

**P7. Native where it matters.** Tauri + Solid for the desktop shell; the daemon
owns every PTY and the only VT engine. We do not recreate an IDE.

---

# 3. Scope

## 3.1 The MVP's main flow

```text
Open the application → the daemon connects or starts
    ↓
Add Project → select ~/code/my-project
    ↓
The project appears in the sidebar (repo, branch)
    ↓
New Session → Terminal | Claude | Codex | OpenCode | Cursor
    ↓
The daemon creates a PTY in the workspace's cwd
    ↓
The CLI runs; the terminal appears and accepts input
```

Also: several simultaneous sessions; switching through the sidebar/tabs; splits; creating a worktree; a session directly in a worktree; closing the GUI without killing sessions; reopening and rebuilding the visible terminal; manually creating a child session.

## 3.2 Included in the MVP

**Projects:** add through a folder picker; remove from the list without deleting files; detect Git; show path, name, branch and status (dirty/clean); persist.

**Workspaces:** main; create/list/delete a managed worktree with validations; open sessions in any of them.

**Terminals:** multiple PTYs; the user's login shell; complete keyboard input; resize; bounded scrollback; selection and copy/paste; ANSI/truecolor; complete TUIs (alternate screen, cursor shapes, basic mouse reporting); persistence while the daemon lives; reattach after restarting the GUI.

**Agents:** four builtins; binary detection with verification; installed/not installed state; manual executable override; start in the workspace's cwd; version recorded; kill/restart; a normal terminal as the universal fallback.

**Sessions:** `Starting/Running/Exited/Failed/Orphaned` states; rename; close vs kill; manual child sessions with a role; persistence and reconciliation.

**UI:** a Project → Workspace → Session sidebar (with child nesting); a central area with tabs and splits; empty states; a command palette; context menus; shortcuts through the shell Actions; an activity indicator on hidden terminals; a notification when an agent finishes.

**Daemon:** autostart; one singleton per user; local IPC; reconnect; metadata persistence; logs; an explicit clean shutdown.

**Configuration:** a minimal `config.toml` (§15.4).

## 3.3 Outside the MVP

An embedded browser; a staging UI; SSH/remote; Windows; web/mobile; ACP as the primary mode; conversation parsing; our own tool approvals; MCP management; provider authentication; cloud sync; a plugin marketplace; an automatic orchestrator; automatic context transfer; unlimited session recording; **recovering PTYs after a daemon crash**.

(The file editor, the diff viewer and PRs left this list when they were
implemented; see ADR-012 and `docs/plan-editor.md`.)

About the last exclusion: the GUI can die and the sessions survive; **if the daemon dies, the PTYs die with it**. The affected sessions are marked `Orphaned` on restart (§15.3). Surviving a daemon crash would require an external tmux/supervisor-like layer.

---

# 4. Conclusions from the prior research

## 4.1 Orca
It validates the target UX: multiple agents, worktrees, terminals, fast navigation, visible status, a dense interface. It also shows the cost of Electron (PTY optimisations, WebGL, memory, startup, idle CPU). **Conclusion:** take the UX and the flows; not the Electron architecture. https://github.com/stablyai/orca

## 4.2 T3 Code
A `Clients → RPC → Server runtime` separation where the server is the execution boundary, with a registry/adapter for providers. **Adopted:** separating the GUI and the runtime; an agent registry; typed contracts; distinguishing project/workspace/session. https://github.com/pingdotgg/t3code

## 4.3 Herd
A simple `Project → Session → tmux pane` model. **Adopted:** a simple hierarchical sidebar; worktree + agent in one step; a runtime that outlives the UI. **Not adopted:** tmux as the runtime (we want direct control of PTYs, snapshots, parent/child relations). https://github.com/allenan/herd

## 4.4 Arbor
Direct prior art: Rust + a desktop shell + a daemon + worktrees + a native terminal + agents. **Conclusion:** the architecture is not theoretical. https://github.com/penso/arbor

## 4.5 Desktop GUI
**Chosen:** Tauri 2 + Solid/Vite. The WebView owns chrome; a Canvas paints the
passive cell grid. Keyboard chords and actions live in the Solid layer; the
Rust host bridges to `client` off the UI thread.

## 4.6 Control kit
Forge-owned controls under `apps/tauri/src/ui/` (look + tokens), with headless
interaction primitives where needed. Theme literals live in
`apps/tauri/src/theme/tokens.ts`.

---

# 5. Architecture decisions (ADRs)

## ADR-001 — Rust for the runtime and host
**Accepted.** Process and PTY control, concurrency, types shared between the daemon and the GUI host, Serde, POSIX. The WebView UI is TypeScript/Solid; it never owns a second VT engine.

## ADR-002 — Tauri 2 + Solid
**Accepted.** Tauri: window, native menus, dialogs, packaging. Solid/Vite: chrome, workbench, Canvas terminal. The terminal grid is painted from a passive `client::CellGrid` replica (ADR-011).

**Dependency policy:**
- Theme colors and metrics come from `apps/tauri/src/theme/tokens.ts`; views must not hardcode hex.
- `cargo deny check licenses` in CI before any distribution.

## ADR-003 — Our own daemon
**Accepted.**

```text
{app}            # GUI
{app}-daemon     # local runtime
```

The daemon owns: PTYs, child processes, sessions, worktree operations, the shell environment, the terminal's state (grid + scrollback), runtime state, domain SQLite writes, event distribution.

The GUI owns: layout, selection/focus, rendering, shortcuts, transient UI state, presentation preferences.

**Derived rule:** no protocol request may represent GUI-exclusive state (focus, active pane, sidebar width). That state is persisted by the GUI in `layout_state` through a generic `SetAppState` request, but the daemon does not interpret it.

## ADR-004 — Local IPC through a Unix Domain Socket
**Accepted for macOS/Linux.**

The socket's path:

```text
Linux:  $XDG_RUNTIME_DIR/{app}/daemon.sock
        fallback: /tmp/{app}-$UID/daemon.sock
macOS:  $TMPDIR/{app}/daemon.sock        # $TMPDIR is per-user on macOS
        fallback: /tmp/{app}-$UID/daemon.sock
```

Rules:
- The directory with mode `0700`; the socket with `0600`.
- **The complete path must measure less than 100 bytes** (the `sun_path` limit of 104 on macOS, 108 on Linux). If it does not fit, use the fallback. It is validated at startup and fails with an explicit error, never silently.
- Do not use `~/Library/Caches` (purgeable) nor `~/Library/Application Support` (too long with long usernames).

**Serialization:** MessagePack through `rmp-serde`, framing with a big-endian `u32` length + the payload. Maximum frame size: **16 MiB**; larger frames close the connection with a protocol error. A `{app}-daemon dump --json` subcommand prints messages in JSON for debugging.

HTTP/WebSocket is not needed in V1.

## ADR-005 — The PTY is owned by the daemon
**Accepted.** Never `GUI → spawn(agent)`. Always `GUI → request → Daemon → PTY → process`. It allows persistence, a single source of truth, future clients, orchestration, centralised logging.

## ADR-006 — Terminal-first model for agents
**Accepted.** The MVP does not turn the agents' output into a chat of its own. A provider defines how to launch a program inside the PTY (see `AgentDescriptor` in §7.5, the single definition).

## ADR-007 — Declarative provider registry
**Accepted.** Two levels:

- **Level 1 — Descriptor:** static data (candidate binaries, args, version verification). The MVP's four builtins are resolved this way.
- **Level 2 — Adapter:** an `AgentAdapter` trait for special behaviour (resume, initial prompts, session detection). **No adapter is implemented in the MVP**; the trait exists with a generic `DescriptorAdapter` implementation wrapping any descriptor.

```rust
pub trait AgentAdapter: Send + Sync {
    fn descriptor(&self) -> &AgentDescriptor;
    fn detect(&self, env: &ResolvedEnvironment) -> DetectionResult;
    fn build_launch(&self, req: &LaunchAgentRequest, env: &ResolvedEnvironment) -> Result<SpawnSpec, AgentError>;
}
```

## ADR-008 — Git CLI first
**Accepted.** The commands used in the MVP, always with `-C <path>` and `--porcelain` where it exists:

```text
git rev-parse --show-toplevel
git rev-parse --abbrev-ref HEAD
git symbolic-ref refs/remotes/origin/HEAD     # default branch (best effort)
git status --porcelain=v2 --branch
git branch --list --format=%(refname:short)
git worktree list --porcelain
git worktree add <path> -b <branch> <base>
git worktree add <path> <existing-branch>
git worktree remove [--force] <path>
git worktree prune
```

Rules: a forced locale (`LC_ALL=C`), `GIT_TERMINAL_PROMPT=0`, a **30 s** timeout per command, never run inside the user's PTY. `git status` runs on demand and throttled (at most once every 2 s per workspace), never in continuous polling.

`gix`/`git2` may be introduced later.

## ADR-009 — SQLite for metadata; not for the terminal stream
**Accepted.** SQLite stores projects, workspaces, sessions, hierarchy, provider overrides, layout, preferences. **Never** terminal chunks. The scrollback lives in the daemon's memory, bounded. Future session recording will go to segmented files.

## ADR-010 — The session graph is separate from the process tree
**Accepted and critical.** A session has a `parent_session_id?` and a `root_session_id`. The logical tree does not imply a process tree: the daemon creates processes as peers and stores the relation in the domain. It allows restarting a child without killing the parent, moving it to another worktree, changing provider, keeping lineage.

**Graph rules (new):**
- `root_session_id == id` for roots.
- Maximum depth in the MVP: **8** (validated in the daemon).
- Any `parent_session_id` that would create a cycle or cross projects is rejected (a child may be in another workspace of the same project, not in another project).
- When closing (Close) a session with children: the children **are re-parented to the grandparent** (or become roots if there is no grandparent) and `root_session_id` is recomputed. They are not closed in cascade. Kill never cascades.

## ADR-011 — The daemon is the single source of truth for the grid; the GUI renders a snapshot + diffs (new)

## ADR-012 — The daemon owns the workspaces' filesystem; the GUI never does `std::fs`

It extends ADR-011's principle to files: listing, reading, writing and searching
go through the daemon (`fs-service`), with canonical paths bounded to the checkout and
writes conditioned on a `revision` (a hash of the content that was read). That way an
agent writing the same path does not get silently overwritten, and a remote daemon
stays possible without redoing the layer. The editor embeds the UI kit's
`InputState::code_editor` (Apache-2.0); Zed's GPL core is left out
(`deny.toml`). It is not an IDE: no LSP, no multi-cursor, no inline staging.
See `docs/plan-editor.md`.
**Accepted.** It resolves the ambiguity of §10.5 v1.

Options evaluated:
- **(A) The GUI replicates bytes:** the daemon keeps a log of bytes + resizes; the GUI runs its own `TerminalEngine` and replays it. Simple on the wire, but the replay has to reproduce resizes in the exact order, the log grows over time and there are two emulators that can diverge.
- **(B) An authoritative daemon, the GUI renders diffs:** the daemon runs the single `TerminalEngine`; on attach it sends a `TerminalSnapshot` (the visible grid + cursor + modes + N scrollback lines) and afterwards `TerminalDelta`s with the damaged rows (damage tracking). The GUI keeps a replica of cells, not an emulator.

**Decision: (B).** `alacritty_terminal` exposes per-row damage (`TermDamage`), which is exactly what is needed. Advantages: a single truth, trivial reattach, a thin GUI, traffic proportional to the visual change and not to the raw output, natural coalescing. Cost: scrollback outside the window is asked for on demand (`FetchScrollback`), and selection/copy over the scrollback uses that request.

Fallback if the spike shows unacceptable input latency (>1 extra frame): keep (B) and add **predictive local echo** in the GUI, never switch to (A).

---

# 6. General architecture

```text
┌──────────────────────────────────────────────────────────┐
│                     the shell Desktop App                     │
│  Project Sidebar   Workspace / Sessions     Command UI   │
│                TerminalView(s)  (a replica of cells)     │
│  the shell + the UI kit        IPC thread (tokio)         │
└───────────────────────────┬──────────────────────────────┘
                            │  UDS / MessagePack / framed
┌───────────────────────────▼──────────────────────────────┐
│                        Daemon (tokio)                    │
│  ProjectService  WorkspaceService  GitService            │
│  SessionService  TerminalService   AgentService          │
│  ShellEnvironmentService  PersistenceService             │
│  ClientRegistry  EventBus                                │
├───────────────┬────────────────┬─────────────────────────┤
▼               ▼                ▼                         ▼
PTYs           git              SQLite                 filesystem
├── zsh/bash/fish
├── claude / codex / opencode / agent
```

**Async runtimes (a new decision):** the daemon uses multi-thread `tokio`. The GUI does not run tokio on the shell's thread: it creates **a dedicated IPC thread** with a `current_thread` tokio runtime, and crosses over to the shell through channels (`async-channel`/`flume`) consumed with `cx.spawn` on the shell's executor. No tokio type crosses into the `ui` crate.

---

# 7. Domain model

Every ID is a newtype over `uuid::Uuid` v7 (time-orderable). Every timestamp is a `time::OffsetDateTime` in UTC, persisted as ISO-8601.

## 7.1 Project

```rust
pub struct Project {
    pub id: ProjectId,
    pub name: String,                 // the root_path's basename by default; editable
    pub root_path: PathBuf,           // canonicalised
    pub git_root: Option<PathBuf>,    // None if it is not a repo
    pub created_at: Timestamp,
    pub last_opened_at: Timestamp,
}
```

A non-Git project is valid; the worktree actions are disabled. Two projects cannot have the same canonical `root_path`. If `root_path` is inside another already registered project, it is allowed but warned about.

## 7.2 Workspace

```rust
pub enum WorkspaceKind { Main, GitWorktree }   // FutureRemote is added when it exists

pub struct Workspace {
    pub id: WorkspaceId,
    pub project_id: ProjectId,
    pub kind: WorkspaceKind,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub managed_by_app: bool,         // true only for worktrees created by {APP}
    pub created_at: Timestamp,
}
```

When adding a Git project, a `Main` Workspace is always created plus one `GitWorktree { managed_by_app: false }` per already existing worktree. The main one is not a special case anywhere else in the code.

## 7.3 Session

```rust
pub enum SessionKind { Shell, Agent }

pub enum SessionState {
    Starting,                         // request accepted, the PTY does not report a live process yet
    Running,
    Exited { code: Option<i32>, signal: Option<i32> },
    Failed { reason: String },        // the spawn failed (the binary does not exist, an invalid cwd, etc.)
    Orphaned,                         // it was Running when the daemon restarted
}

pub enum SessionRole {
    Generic, Orchestrator, Planner, Researcher, Executor, Reviewer, Tester,
    Custom(String),
}

pub struct Session {
    pub id: SessionId,
    pub workspace_id: WorkspaceId,
    pub kind: SessionKind,
    pub role: SessionRole,
    pub parent_session_id: Option<SessionId>,
    pub root_session_id: SessionId,
    pub terminal_id: Option<TerminalId>,       // None in Failed/Orphaned or after a pending restart
    pub agent_provider_id: Option<AgentProviderId>,
    pub title: SessionTitle,
    pub state: SessionState,
    pub created_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

pub struct SessionTitle {
    pub user: Option<String>,         // set by RenameSession; it takes precedence
    pub terminal: Option<String>,     // the last OSC 0/2 received
}
```

**Title rule:** `user` is shown, otherwise `terminal`, otherwise `"{provider}"` or `"Shell"`.

**Valid transitions:**

```text
Starting → Running | Failed
Running  → Exited | Orphaned (only through reconciliation)
Exited | Failed | Orphaned → Starting   (through RestartSession: same SessionId, new TerminalId)
```

**Close vs Kill:**
- `KillSession`: ends the process (SIGTERM, a **3 s** grace, SIGKILL). The session goes to `Exited` and **stays in the sidebar** until Close.
- `CloseSession`: removes the session from the model and from the UI. Only allowed if the state is not `Running`/`Starting`; if it is, the GUI must confirm and then sends `KillSession` followed by `CloseSession`. ADR-010's re-parenting rule applies.
- `RestartSession`: only from terminal states; it reuses the kind, role, provider, workspace and parent.

## 7.4 TerminalRuntime (daemon only, not serializable)

```rust
pub struct TerminalRuntime {
    pub id: TerminalId,
    pub session_id: SessionId,
    pub pty: Box<dyn PtyHandle>,
    pub child_pid: u32,
    pub process_group: i32,
    pub engine: Box<dyn TerminalEngine>,
    pub seq: u64,                              // increments per emitted delta
    pub size: PtySize,
    pub subscribers: HashMap<ClientId, SubscriberState>,
}
```

## 7.5 AgentDescriptor (the single definition)

```rust
pub struct AgentDescriptor {
    pub id: AgentProviderId,                          // "claude" | "codex" | "opencode" | "cursor"
    pub display_name: &'static str,
    pub binary_candidates: &'static [&'static str],   // in order of preference
    pub default_args: &'static [&'static str],
    pub version_probe: VersionProbe,                  // how to verify the binary is the right one
    pub capabilities: AgentCapabilities,
}

pub struct VersionProbe {
    pub args: &'static [&'static str],                // typically ["--version"]
    pub expect_substring: Option<&'static str>,       // e.g. "cursor", to rule out an unrelated `agent`
    pub timeout_ms: u32,                              // 3000
}

pub struct AgentCapabilities {
    pub interactive_tui: bool,        // true for the four builtins
    pub supports_initial_prompt: bool,// informative in the MVP, unused
    pub supports_resume: bool,        // informative in the MVP, unused
}
```

Builtins:

| id | display | candidates | version probe | expect |
|----|---------|------------|---------------|--------|
| `claude` | Claude Code | `claude` | `--version` | — |
| `codex` | Codex CLI | `codex` | `--version` | — |
| `opencode` | OpenCode | `opencode`, `opencode2` | `--version` | — |
| `cursor` | Cursor CLI | `agent`, `cursor-agent` | `--version` | `cursor` (case-insensitive) |

`opencode2` is included as a lower-priority candidate; it is not an MVP dependency. For Cursor, `agent` is accepted **only** if the probe contains "cursor"; otherwise it moves on to the next candidate.

## 7.6 Shared runtime types (defined in `terminal-core` and `domain`)

```rust
pub struct PtySize { pub cols: u16, pub rows: u16, pub pixel_width: u16, pub pixel_height: u16 }

pub struct SpawnSpec {
    pub program: PathBuf,                 // an absolute, already resolved path
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,       // the complete environment, not incremental
}

pub struct ResolvedEnvironment {
    pub shell: PathBuf,
    pub vars: Vec<(String, String)>,
    pub path_entries: Vec<PathBuf>,
    pub resolved_at: Timestamp,
    pub source: EnvSource,                // LoginShell | ProcessFallback
}

pub struct LaunchAgentRequest {
    pub provider_id: AgentProviderId,
    pub cwd: PathBuf,
    pub extra_args: Vec<String>,          // empty in the MVP
    pub executable_override: Option<PathBuf>,
}

pub struct DetectionResult {
    pub provider_id: AgentProviderId,
    pub status: DetectionStatus,          // Installed { executable, version } | NotFound | Rejected { candidate, reason } | ProbeTimeout
    pub checked_at: Timestamp,
}
```

`TerminalSnapshot` and `TerminalDelta` are defined in §11.4.

---

# 8. Session graph and future orchestration

## 8.1 Relations
A future example: Session A (Claude, Orchestrator, main) with children B (Claude, Planner, main), C (Codex, Executor, worktree/auth), D (OpenCode, Reviewer, worktree/auth). The parent can create children with different workspace policies.

## 8.2 ChildWorkspacePolicy

```rust
pub enum ChildWorkspacePolicy {
    SameWorkspace,
    NewManagedWorktree { branch_hint: Option<String>, base: Option<String> },
    ExistingWorkspace(WorkspaceId),
}
```

In the MVP the "New child session" UI exposes `SameWorkspace` and `ExistingWorkspace` (a selector). `NewManagedWorktree` exists in the protocol and is implemented in the daemon (creating the worktree + the session in a single operation) but the UI may expose it in M6 if there is time; it is not an exit criterion.

## 8.3 ContextEnvelope

```rust
pub struct ContextEnvelope {
    pub id: ContextId,
    pub source_session_id: SessionId,
    pub target_session_id: Option<SessionId>,
    pub summary: Option<String>,
    pub instructions: Option<String>,
    pub artifacts: Vec<ContextArtifactRef>,
    pub git_context: Option<GitContextRef>,
    pub created_at: Timestamp,
}

pub enum ContextArtifactKind {
    Text, Plan, Review, FileReference, DiffReference, CommitReference, TerminalExcerpt, StructuredJson,
}
```

**MVP:** the schema, the table and the `CreateContextEnvelope` request are defined with only `summary` and `instructions`. There is no UI for sending or consuming them. The goal: that the database migration and the type exist before orchestration.

## 8.4 The future automation channel
`SpawnChildSession` from an agent will arrive through our own CLI, local MCP, ACP, an SDK socket or hooks. **It is not decided yet.** The protocol's `CreateChildSession` request (§10.2) is the only entry point and any future channel reuses it.

---

# 9. Daemon

## 9.1 Lifecycle

**GUI startup (`connect_or_spawn_daemon`):**

```text
1. Resolve the socket path (§ADR-004). Validate its length.
2. Try connect + Hello/HelloAck (a 2 s timeout).
3. If connect → ECONNREFUSED or ENOENT:
      a. If the socket file exists but nobody listens → unlink (stale).
      b. Spawn the daemon (see below).
      c. Retry connect every 100 ms for up to 10 s.
4. If HelloAck.protocol_version != the expected one → show a dialog
   "Daemon incompatible" with a "Restart daemon" action (it sends StopDaemon
   to the old version if it accepts the command; otherwise, SIGTERM by the lockfile's PID).
5. GetSnapshot → render the UI.
```

**Spawning the daemon from the GUI:** the `{app}-daemon` binary is located next to the GUI's executable (`Contents/MacOS/` on macOS; the same directory or `libexec/` on Linux). It is launched with a new session (`setsid`), stdin `/dev/null`, stdout/stderr redirected to `daemon.log`, and **without inheriting the GUI's environment** beyond `HOME`, `USER`, a minimal `PATH`, `XDG_*` and `TMPDIR`; the daemon resolves its own environment (§12).

**Double-spawn race:** if two GUIs start at the same time, both may launch a daemon. The second daemon fails to acquire the lock (§9.2) and exits with code 0 (it is not an error). The GUIs simply retry the connect.

**Closing the GUI:** disconnect → the daemon marks the client as disconnected, removes its subscriptions, and carries on. PTYs and agents stay alive.

**The daemon only terminates because of:**
- An explicit `StopDaemon` (the "Stop daemon and all sessions" UI command, with a confirmation showing the number of live sessions).
- SIGTERM/SIGINT (a system shutdown): killing every session with a 3 s grace, flushing SQLite, exiting.
- A crash.

**Optional auto-exit (a decision):** the daemon does **not** close itself when there are no clients and no sessions. It is explicit by design; it can be revisited post-MVP with a config option.

## 9.2 Singleton

- A `daemon.lock` lockfile next to the socket, acquired with `flock(LOCK_EX | LOCK_NB)`. Its content is JSON `{ pid, instance_id, version, started_at }`, informative; the authority is the `flock`, not the PID.
- If the lock is not obtained: exit with code 0.
- On obtaining the lock: unlink a stale socket if there is one, bind, `chmod 0600`.

Handshake:

```rust
Hello    { protocol_version: u32, client_version: String, client_kind: ClientKind }
HelloAck { protocol_version: u32, daemon_version: String, instance_id: Uuid, started_at: Timestamp }
HelloReject { daemon_protocol_version: u32, reason: String }
```

`protocol_version` is a single integer. In the MVP the GUI and the daemon ship together and exact equality is required; the N/N-1 compatibility policy is decided post-MVP.

## 9.3 Internal services

```text
Daemon
├── ClientRegistry          connections, per-client subscriptions
├── EventBus                broadcast to clients (bounded per client)
├── PersistenceService      SQLite, migrations, repositories
├── ShellEnvironmentService the login environment, cache
├── GitService              a git CLI wrapper
├── ProjectService
├── WorkspaceService
├── AgentService            registry + detection + overrides
├── TerminalService         runtimes per TerminalId, the PTY loop, the engine
└── SessionService          lifecycle, graph, reconciliation
```

Every service is a struct with its own state behind `Arc<Mutex>`/actors with channels, never a global `AppState`. The dependencies go one way: `SessionService → TerminalService → (PtyBackend, TerminalEngine)`; `SessionService → AgentService → ShellEnvironmentService`; `WorkspaceService → GitService`.

---

# 10. IPC and protocol

## 10.1 Model

```rust
pub enum ClientMessage {
    Hello(Hello),
    Request { request_id: u64, body: Request },
}

pub enum DaemonMessage {
    HelloAck(HelloAck),
    HelloReject(HelloReject),
    Response { request_id: u64, body: Result<Response, ProtocolError> },
    Event(DaemonEvent),
}

#[non_exhaustive]
pub struct ProtocolError { pub code: ErrorCode, pub message: String, pub details: Option<String> }

#[non_exhaustive]
pub enum ErrorCode {
    InvalidRequest, NotFound, Conflict, PreconditionFailed,   // e.g. CloseSession on a Running one
    GitError, SpawnError, IoError, ProviderNotInstalled, ProtocolViolation, Internal,
}
```

Every protocol enum is `#[non_exhaustive]` and uses `#[serde(other)]`/`Unknown` variants where applicable to tolerate new fields.

## 10.2 MVP requests

```text
Global
  GetSnapshot                       → Snapshot { projects, workspaces, sessions, providers, app_state }
  StopDaemon { kill_sessions: bool }
  GetAppState { key } / SetAppState { key, value }   # opaque to the daemon

Projects
  AddProject { path }
  RemoveProject { project_id, policy: RemoveProjectPolicy }
  RefreshProject { project_id }     # re-detects git, branch, worktrees
  RenameProject { project_id, name }

Workspaces
  ListWorkspaces { project_id }
  CreateWorktree { project_id, branch, base: Option<String>, name: Option<String> }
  RemoveWorktree { workspace_id, force: bool }
  RefreshWorkspaceStatus { workspace_id }   # git status on demand

Sessions
  CreateShellSession { workspace_id, parent: Option<SessionId>, role }
  CreateAgentSession { workspace_id, provider_id, parent: Option<SessionId>, role }
  CreateChildSession { parent_session_id, kind, provider_id?, role, workspace_policy }
  KillSession { session_id }
  CloseSession { session_id }
  RestartSession { session_id }
  RenameSession { session_id, title: Option<String> }   # None clears the user title
  SetSessionRole { session_id, role }
  CreateContextEnvelope { envelope }

Terminals
  AttachTerminal { terminal_id, size: PtySize }          → AttachAck { snapshot }
  DetachTerminal { terminal_id }
  WriteTerminalInput { terminal_id, bytes }
  ResizeTerminal { terminal_id, size }
  FetchScrollback { terminal_id, from_line, count }      → ScrollbackRows
  SendSignal { session_id, signal }                      # SIGINT/SIGTERM/SIGHUP/SIGKILL

Agents
  ListAgentProviders
  RefreshAgentDetection { provider_id: Option<AgentProviderId> }
  SetProviderExecutable { provider_id, path: Option<PathBuf> }   # an override; None removes it
```

`RemoveProjectPolicy`:

```rust
pub enum RemoveProjectPolicy {
    KeepEverything,          // removes it from the list; worktrees and branches stay on disk. Rejected if there are Running sessions.
    KillSessionsKeepWorktrees,
    KillSessionsRemoveManagedWorktrees,   // only managed_by_app worktrees; it never deletes branches
}
```

**Removed compared to v1:** `FocusSession` (GUI state, ADR-003). **Removed:** the generic `Subscribe/Unsubscribe`; the terminal subscription is `AttachTerminal`, and the domain events (projects/sessions) are always sent to every connected client because their volume is low.

## 10.3 MVP events

```text
ProjectAdded / ProjectUpdated / ProjectRemoved
WorkspaceCreated / WorkspaceUpdated / WorkspaceRemoved
SessionCreated / SessionUpdated          # SessionUpdated covers state, title, role and parent changes
TerminalDelta { terminal_id, seq, rows, cursor, modes }     # only to subscribers
TerminalResync { terminal_id, snapshot }                    # when the client fell behind (§10.5)
TerminalActivity { terminal_id }          # to NON-subscribers, coalesced to 1/s, for the unread indicator
TerminalBell { terminal_id }
AgentDetectionChanged { results }
DaemonNotice { level, message }           # Warning/Error, to be shown as a notification
DaemonShuttingDown { reason }
```

v1's `SessionExited` is merged into `SessionUpdated { state: Exited {..} }` so there is a single session-update path.

## 10.4 Terminal subscriptions

The daemon always drains every PTY. A client receives `TerminalDelta` only for the terminals it did `AttachTerminal` on. The GUI attaches to the **visible** terminals (in an active pane or the selected tab) and detaches when hiding them. A terminal can have multiple subscribers (future multi-client).

**Size with multiple clients:** in the MVP there is a single client; the last `ResizeTerminal` wins. It is documented as a limitation.

## 10.5 Attach, sequences and backpressure

```text
GUI                               DAEMON
 ├── AttachTerminal(size) ───────►│ applies the resize if it differs
 │◄── AttachAck(snapshot seq=N) ──┤
 │◄── TerminalDelta(seq=N+1) ─────┤
 │◄── TerminalDelta(seq=N+2) ─────┤
```

- `seq` is per terminal, monotonic. The GUI discards deltas with `seq <= the last applied one` and, if it receives `seq > the last + 1`, asks to re-attach (it should not happen; it is logged as a bug).
- **A bounded per-subscriber queue of 256 deltas.** If it fills up (a slow GUI), the daemon discards the whole queue and enqueues a single `TerminalResync` with a fresh snapshot. It never blocks the PTY loop nor grows without bound.
- **Coalescing:** the PTY loop feeds the engine continuously; deltas are emitted at most every **8 ms** (or immediately if the queue was empty and there is damage). A row that changed several times within the window is sent once.
- Massive output (`yes`): the engine processes everything; the GUI receives ~120 deltas/s at most, each one with the damaged rows. That is what keeps the render stable.

---

# 11. Terminal subsystem

## 11.1 Layers

```text
PtyBackend  ── bytes ──►  TerminalEngine  ── damage ──►  TerminalDelta  ── IPC ──►  GUI CellGrid  ──►  TerminalView
```

## 11.2 PTY

The first option: `portable-pty`. The criterion for replacing it with our own implementation using `nix`/`rustix` in Phase 0: if it does not allow (a) `setsid` + a controlling TTY, (b) obtaining the child's pgid, (c) `TIOCSWINSZ` with a pixel size, (d) closing inherited fds. If it does, it stays.

```rust
pub trait PtyBackend: Send + Sync {
    fn spawn(&self, spec: &SpawnSpec, size: PtySize) -> Result<Box<dyn PtyHandle>, PtyError>;
}

pub trait PtyHandle: Send {
    fn reader(&mut self) -> Box<dyn Read + Send>;
    fn writer(&mut self) -> Box<dyn Write + Send>;
    fn resize(&mut self, size: PtySize) -> Result<(), PtyError>;
    fn child_pid(&self) -> u32;
    fn process_group(&self) -> i32;
    fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError>;
}
```

The PTY loop runs on a blocking thread per terminal (`spawn_blocking` or a dedicated thread), reads in 64 KiB buffers and hands over to the engine under the runtime's lock.

## 11.3 Process groups and kill

Every session is created with `setsid()`, so the child leads its own group. `KillSession`:

```text
kill(-pgid, SIGHUP)  if the session is a Shell   (the natural behaviour of closing a terminal)
kill(-pgid, SIGTERM) if the session is an Agent
  ↓ wait 3 s (configurable: sessions.kill_grace_ms)
kill(-pgid, SIGKILL) if it is still alive
  ↓ reap; state Exited { signal }
```

Never `kill(pid)` on the individual process: it would leave orphan grandchildren.

## 11.4 Terminal engine

```rust
pub trait TerminalEngine: Send {
    fn feed(&mut self, bytes: &[u8]);
    fn resize(&mut self, size: PtySize);
    fn take_damage(&mut self) -> Damage;             // rows damaged since the last call, or Full
    fn snapshot(&self, scrollback_tail: usize) -> TerminalSnapshot;
    fn rows(&self, range: Range<i64>) -> Vec<Row>;   // negative indices = scrollback
    fn cursor(&self) -> Cursor;
    fn modes(&self) -> TermModes;                    // alt screen, bracketed paste, mouse modes, app cursor keys
    fn title(&self) -> Option<&str>;
    fn take_bell(&mut self) -> bool;
}

pub struct TerminalSnapshot {
    pub seq: u64,
    pub size: PtySize,
    pub visible: Vec<Row>,            // rows lines
    pub scrollback_tail: Vec<Row>,    // the last N scrollback lines (N = 200 by default)
    pub scrollback_len: u64,
    pub cursor: Cursor,
    pub modes: TermModes,
    pub title: Option<String>,
}

pub struct TerminalDelta {
    pub seq: u64,
    pub rows: Vec<(u16, Row)>,        // (visible index, content)
    pub scrolled_lines: u32,          // how many lines entered the scrollback since the previous delta
    pub cursor: Cursor,
    pub modes: TermModes,
}

pub struct Row { pub cells: Vec<Cell>, pub wrapped: bool }
pub struct Cell { pub text: CompactString, pub fg: Color, pub bg: Color, pub flags: CellFlags }   // text holds the complete grapheme; the continuation cells of wide chars carry the WIDE_SPACER flag
```

**Implementation:** `alacritty_terminal` (Apache-2.0), using `Term<EventProxy>` + `vte::ansi::Processor` directly. Its own `EventLoop`/`tty` is **not** used; the PTY is ours. The damage is obtained from `Term::damage()` and reset with `reset_damage()`. An `engine-alacritty` feature flag (the default) is kept so Ghostty VT or another engine can be evaluated without touching `SessionService` or `ui`.

**Scrollback:** **10,000 lines** per terminal by default (`terminal.scrollback_lines` in the config, a maximum of 100,000). It is confirmed with the Phase 9 benchmark; it is not infinite.

**OSC 52 (clipboard from the program):** disabled by default in the MVP.

## 11.5 the shell TerminalView

The GUI keeps a `CellGrid { visible: Vec<Row>, scrollback_cache: BTreeMap<i64, Row>, cursor, modes }` per attached terminal. `TerminalView` renders:

```text
CellGrid → visible rows (or the scroll window) → style runs per row → text layout → GPU draw
```

Rules: one `ShapedLine` per row, cached and only re-shaped if the row changed; the cursor and the selection as overlays; grouped background runs; a virtual scrollbar over `scrollback_len`; when scrolling back, rows outside `scrollback_cache` are asked for with `FetchScrollback` in blocks of 200.

**Font:** a monospace font with an OFL licence (JetBrains Mono or equivalent) is bundled as the default to avoid fontconfig differences between distros; the user can change it in the config.

## 11.6 Input

The keyboard → bytes mapping lives in `terminal-core::input`, pure and testable, dependent on `TermModes` (app cursor keys, bracketed paste). MVP coverage: printables, arrows, Home/End/PgUp/PgDn, F1–F12, Ctrl+letter, Alt/Meta (an ESC prefix), Backspace/Delete, Tab/Shift+Tab, paste (bracketed if the mode is active), mouse reporting modes 1000/1002/1006 (SGR). The Kitty keyboard protocol: outside the MVP.

**IME:** a requirement before the general release (Phase 9), not for M1. It is recorded as a risk on Linux.

The critical path: `key event → TerminalView → IPC thread → WriteTerminalInput → daemon → PTY`, without going through global state.

---

# 12. Shell environment

A GUI launched from Finder/a launcher does not inherit an interactive shell's `PATH`. `ShellEnvironmentService`:

1. Detect the shell: `$SHELL`; if empty, the `getpwuid` entry; fallback `/bin/sh`.
2. Run `<shell> -l -c 'printf "__{APP}_ENV_BEGIN__"; env -0; printf "__{APP}_ENV_END__"'` with a **5 s timeout**, stdin `/dev/null`, no TTY. `env -0` avoids the ambiguity of multiline values. For `fish` the command is the same (it supports `-l -c`).
3. Parse between the sentinels; ignore everything else (banners, messages from rc files).
4. If it fails or times out: `source: ProcessFallback` with the daemon's environment + a `PATH` widened with known directories (`/usr/local/bin`, `/opt/homebrew/bin`, `~/.local/bin`, `~/.cargo/bin`, `~/.bun/bin`, `~/.npm-global/bin`), and a warning `DaemonNotice`.
5. Cache it; a manual refresh (`RefreshAgentDetection`) or when `$SHELL` changes.

Variables excluded from the environment passed to the PTYs and from the logs: none is removed (the CLIs need their tokens), but **the logs only record variable names, never values**, and `PATH` is logged in full because it is diagnostic.

---

# 13. Agents

## 13.1 Detection

For each descriptor, in candidate order:

```text
1. If there is a user override → use it and skip to 3.
2. Look for the candidate in the ResolvedEnvironment's path_entries.
3. Run version_probe with a timeout; capture stdout+stderr.
4. If expect_substring is defined and does not appear → Rejected, next candidate.
5. Installed { executable, version: the probe's first line }.
```

It runs: when the daemon starts (in the background, without blocking the snapshot), on `RefreshAgentDetection`, and when an override changes. **Never** on every opening of the picker nor on render. The results go in `AgentDetectionChanged`.

## 13.2 Agent picker

```text
New Session
  Terminal
Agents
  Claude      Installed · 1.x
  Codex       Installed
  OpenCode    Not found   [Set path…]
  Cursor      Installed (agent)
```

A provider that is not installed blocks nothing; "Set path…" calls `SetProviderExecutable`. No automatic installers.

## 13.3 Launching

`CreateAgentSession` → `AgentService::build_launch` produces a `SpawnSpec` with an absolute `program`, `args = default_args`, `cwd = workspace.path`, `env = ResolvedEnvironment.vars` + `TERM=xterm-256color` + `COLORTERM=truecolor` + `{APP}_SESSION_ID=<id>` + `{APP}_WORKSPACE=<path>`. The last two let future integrations (our own CLI, MCP) identify the session from inside.

## 13.4 Custom agents (post-MVP)

```toml
[[agents]]
id = "my-agent"
name = "My Agent"
commands = ["my-agent"]
args = []
version_args = ["--version"]
```

The format is defined now; the parser and the UI are post-MVP.

---

# 14. Git and worktrees

## 14.1 Project discovery

`git -C <path> rev-parse --show-toplevel`. If it works: `git_root`, branch, worktrees. If it fails: the project is usable as a folder with no Git actions. If the path is **inside a worktree** of another repo, `git_root` points at the worktree; it is accepted as-is.

## 14.2 Location and slug

```text
macOS: ~/Library/Application Support/{APP}/worktrees/<project-id>/<slug>
Linux: $XDG_DATA_HOME/{APP}/worktrees/<project-id>/<slug>
```

`slug` = the branch with `/` → `-`, characters outside `[A-Za-z0-9._-]` → `-`, repeated `-` collapsed, max. 64 chars. On collision → a `-2`, `-3` suffix. The user sees the real path in the details. The parent directory is created with `0700`. Configurable with `worktrees.root` in the config.

## 14.3 Creating

```text
validate the branch (git check-ref-format --branch) and that the slug does not exist
↓
if the branch does NOT exist:  git worktree add <path> -b <branch> <base>   (default base: the main's HEAD)
if the branch exists and is not checked out in another worktree:  git worktree add <path> <branch>
if it is checked out somewhere else:  a Conflict error with the path where it is
↓
Workspace { kind: GitWorktree, managed_by_app: true }
↓
WorkspaceCreated
```

## 14.4 Deleting

Pre-checks (returned in the answer so the GUI can confirm): `Running` sessions in that workspace, a non-empty `git status --porcelain`, a merge/rebase in progress.

With `force: false`: reject with `PreconditionFailed` if any check fails. With `force: true`: kill the sessions (the same policy as `KillSession`), `git worktree remove --force`, `git worktree prune`. The branch is **never** deleted in the MVP. Worktrees with `managed_by_app: false` can only be removed from the model, not from disk.

---

# 15. Persistence

## 15.1 Paths (`directories`)

```text
config:  ~/Library/Application Support/{APP}/config.toml   |  $XDG_CONFIG_HOME/{APP}/config.toml
data:    ~/Library/Application Support/{APP}/               |  $XDG_DATA_HOME/{APP}/
         ├── app.db
         ├── worktrees/
         └── logs/  (app.log, daemon.log, 5×10 MB rotation)
runtime: socket + lockfile (ADR-004)
```

## 15.2 Initial schema (migrations with `rusqlite_migration`; WAL; foreign keys ON)

```sql
projects   (id TEXT PK, name, root_path UNIQUE, git_root NULL, created_at, last_opened_at)
workspaces (id TEXT PK, project_id FK, kind, path UNIQUE, branch NULL, managed_by_app INT, created_at)
sessions   (id TEXT PK, workspace_id FK, kind, role, parent_session_id NULL FK, root_session_id FK,
            agent_provider_id NULL, user_title NULL, terminal_title NULL, last_state, last_exit_code NULL,
            created_at, ended_at NULL)
context_envelopes (id TEXT PK, source_session_id FK, target_session_id NULL FK, summary NULL,
            instructions NULL, artifacts_json, git_context_json NULL, created_at)
provider_overrides (provider_id TEXT PK, executable_path)
app_state  (key TEXT PK, value TEXT)     -- the UI kit's serialized layout, sidebar width, last active session
schema_version (version INT)
```

`terminal_id` is **not** persisted: it is pure runtime and it is regenerated on every spawn/restart.

## 15.3 Reconciliation when the daemon starts

1. Migrate the schema.
2. `UPDATE sessions SET last_state='Orphaned', ended_at=now WHERE last_state IN ('Starting','Running')`.
3. Load projects/workspaces; validate that the paths exist; the ones that do not are flagged with a `DaemonNotice` but not deleted.
4. Re-run `git worktree list` per project to detect worktrees added/deleted outside the app.
5. Emit a consistent snapshot.

Never pretend a PTY survived a daemon restart.

## 15.4 `config.toml` (MVP)

```toml
[terminal]
scrollback_lines = 10000
font_family = "JetBrains Mono"
font_size = 13.0

[sessions]
kill_grace_ms = 3000
shell = ""              # empty = $SHELL

[worktrees]
root = ""               # empty = the §14.2 default

[daemon]
log_level = "info"
```

The GUI reads the `[terminal]` keys; the daemon reads `[sessions]`, `[worktrees]`, `[daemon]`. Changes require a restart in the MVP (no hot reload).

---

# 16. UI/UX

## 16.1 Visual direction
Orca as a reference, not a copy. High density, minimal chrome, an always-useful sidebar, the terminal taking almost everything, contextual actions, keyboard-first, visible status without noise.

## 16.2 Main layout

```text
┌──────────────────────────────────────────────────────────────┐
│ Project / Workspace                + Session      Commands    │
├───────────────┬──────────────────────────────────────────────┤
│ PROJECTS      │   Session tabs / panes                       │
│ ▼ project-a   │   ┌──────────────────────────────────────┐   │
│   main        │   │              terminal                │   │
│    ● Claude   │   └──────────────────────────────────────┘   │
│    $ shell    │                                              │
│   auth        │                                              │
│    ● Codex    │                                              │
│ ▶ project-b   │                                              │
├───────────────┴──────────────────────────────────────────────┤
│ branch / workspace      sessions      daemon      connection │
└──────────────────────────────────────────────────────────────┘
```

## 16.3 Sidebar

```text
Project
├── Main
│   ├── ● Claude · auth refactor
│   │   └── ○ Planner (Claude)         ← a child, indented
│   ├── ● Codex · tests
│   └── $ Terminal
└── feature/auth
    └── ● Cursor
```

Status icons: `●` Running, `○` Starting, `◌` Exited (grey), `✕` Failed/Orphaned, an activity dot when a hidden terminal produces output. Visual nesting up to 3 levels; deeper than that it collapses into "+N".

## 16.4 the UI kit components

Sidebar/Tree, Tabs, Dock/Tiles (splits), Resizable, Menu, Popover, Dialog, Notification, VirtualList, Tooltip. Do not adopt large components that are not needed.

## 16.5 Empty states
No projects: `No projects yet — [Add Project]`. A project with no sessions: `Start a session — [Terminal] [Claude] [Codex] [OpenCode] [Cursor]`.

## 16.6 Shortcuts (the shell Actions + KeyContext; `Cmd` on macOS, `Ctrl` on Linux)

```text
Cmd+K            Command palette
Cmd+T            New terminal (in the selected workspace)
Cmd+Shift+A      New agent
Cmd+W            Close current session (with a confirmation if Running)
Cmd+1..9         Focus tab N
Cmd+Shift+[ / ]  Previous / next session
Cmd+B            Toggle sidebar
Cmd+D / Cmd+Shift+D   Split right / down
Cmd+Shift+C / V  Copy / paste (in TerminalView; Cmd+C/V on macOS)
```

KeyContexts: `App`, `Sidebar`, `Terminal`, `CommandPalette`. The keys in the `Terminal` context go to the PTY except the ones listed above.

## 16.7 Working tree diff

A review view, not a Git client: it answers "what has the agent changed in this
checkout?" and it applies nothing. It opens from the `+` menu of the tab strip
and from a branch's menu in the rail, and it occupies a tab of its own —
there is no session behind it, because it belongs to the *checkout*.

A tree of changed paths on the left (single-child directories collapsed into one
row), the selected file's patch on the right: two columns when the panel has
room, stacked when it does not. A single source: the working tree against
`HEAD` — staged, unstaged and untracked.

```text
tab      switch half               m   mark as reviewed
n / p    next/previous file        v   toggle two columns
] / [    next/previous hunk        r   re-read   esc  close
```

## 16.8 PR tab with an agent

Sibling of the Diff one, and it does not replace `Open PR…` in the rail: that
stays the short path. This one is the path with an agent inside.

Always visible: the branch, how many files and how many lines, and the button.
The prompt goes **folded behind a gear**, because the normal case is not reading
it. Unfolding it shows in full what is going to be sent, and below it the box
for what gets added — "extend" is visibly an append, never a replacement.

The template decides what the agent does, and that is why the mode is a field of
it and not a separate switch:

- `Draft` — the agent only writes the title and the body; Forge pushes and opens
  the PR through the path that already exists. It does not touch the working tree.
- `Implement` — the agent works, commits, pushes and opens the PR itself.
  Forge only launches it and watches.

The progress is the state of the launched session, linked, not duplicated. The
link to the PR is **not** scraped from the terminal's output: the pull request
whose `head_ref` is this checkout's branch is looked up, which answers the same
no matter who opened it.

---

# 17. Repository structure

```text
repo/
├── Cargo.toml  Cargo.lock  rust-toolchain.toml  deny.toml  README.md
├── docs/  architecture.md  protocol.md  terminal.md  agents.md  worktrees.md  development.md
├── crates/
│   ├── app/            src/main.rs                 # the GUI binary: startup, connect_or_spawn_daemon, config loading
│   ├── theme tokens/  src/{theme,actions,icons,compat}.rs
│   ├── ui/             src/{app_shell,sidebar,sessions,terminal,command_palette,dialogs}/
│   ├── client/         src/{ipc_thread,store}.rs   # the protocol client + the state replica (CellGrid, snapshot) — without the shell
│   ├── daemon/         src/main.rs  runtime.rs  client_registry.rs  services/{projects,workspaces,sessions,terminals,agents,git,environment}.rs
│   ├── protocol/       src/{framing,hello,request,response,event,error}.rs
│   ├── domain/         src/{ids,project,workspace,session,agent,context}.rs
│   ├── terminal-core/  src/{pty,engine,alacritty_engine,snapshot,input}.rs
│   ├── agents/         src/{registry,descriptor,detection,builtins}.rs
│   ├── git-service/    src/{command,repository,worktree}.rs
│   ├── persistence/    src/{db,migrations,repositories}/
│   └── test-support/   src/{fake_agent,fake_pty,temp_repo}.rs
└── scripts/  dev  package-macos  package-linux
```

**Dependencies between crates (one direction):**

```text
app → ui → client → protocol → domain
daemon → {agents, git-service, persistence, terminal-core} → domain
daemon → protocol
ui → theme tokens → the shell, the UI kit
```

`ui` never depends on `terminal-core` (the GUI does not emulate); `client` is the only one that knows the protocol on the GUI side. The daemon's `services/*.rs` are thin orchestration over the crates (`services/git.rs` uses `git-service`; `services/agents.rs` uses `agents`).

A practical start: create `domain`, `protocol`, `terminal-core`, `daemon`, `client`, `app` on day one. Extract `agents`, `git-service`, `persistence` when they exceed ~500 lines inside `daemon`.

---
---

# 18. Execution plan

A corrected order: the terminal comes before Projects (coherent with §28 and §36). Every phase ends with **verifiable** criteria.

## Phase 0 — Technical spike

**0.1 the shell + the UI kit:** a window with a sidebar, a resizable area, tabs, a custom panel. Validate on macOS, Linux Wayland and **Linux X11 (required)**.

**0.2 Minimal terminal:** PTY + `alacritty_terminal` + a TerminalView with damage → deltas (ADR-011 already applied in the spike). Validate `echo`, `ls --color`, `vim`, `htop`, resize, alt screen, copy/paste.

**0.3 TUI agents:** run `claude`, `codex`, `opencode`, `agent`/`cursor-agent` and verify render and input. No authentication in CI.

**0.4 Daemon + reconnect:** GUI → daemon → PTY; close the GUI while a script produces output; reopen; verify the grid matches what the daemon's engine shows.

**0.5 Latency (new):** measure key → echo on screen with ADR-011. Target ≤ 2 frames (≈33 ms) on the reference machine.

**Gate:** do not move on if the TUIs fail, reattach is not viable, the shell/Linux has blockers, or 0.5's latency exceeds 50 ms with no identified solution.

## Phase 1 — Foundation
The Cargo workspace, the toolchain, CI (macOS + headless Linux); typed UUIDv7 IDs; `tracing` with `app.log`/`daemon.log`; paths; `connect_or_spawn_daemon`; lock + socket + handshake; `config.toml`.
**Exit:** opening the GUI starts/connects the daemon; the status bar shows the `instance_id` and the version; killing the daemon externally shows "Disconnected" and reconnecting works.

## Phase 2 — Terminal runtime (previously Phase 3)
PtyBackend; TerminalEngine; TerminalService; TerminalView; input mapping; resize; scrollback + FetchScrollback; attach/detach; seq + resync; backpressure.
**Exit:** a shell in `$HOME` comfortably usable; `vim`/`less`/`htop` correct; closing the GUI with `yes | head -c 50M` in progress and reopening shows a consistent state; a full queue produces a `TerminalResync` and no memory growth (a test).

## Phase 3 — Projects (previously Phase 2)
A folder picker; ProjectService (canonicalize, dedupe); Git detection; persistence; the sidebar.
**Exit:** adding, closing/reopening the GUI and the daemon, and seeing the projects with the right branch; adding the same path twice returns `Conflict`.

## Phase 4 — Session model
SessionService separate from TerminalRuntime; the §7.3 states and transitions; Close/Kill/Restart; multiple sessions; tabs; splits with Dock/Tiles; rename; persistence + `Orphaned` reconciliation.
**Exit:** 6 shells in one workspace organised into tabs and 2 splits; restarting the daemon marks them all `Orphaned` and `RestartSession` relaunches them with the same `SessionId`.

## Phase 5 — Agent providers
The registry; ShellEnvironmentService; verified detection; the picker; the executable override; launching all four; metadata (icon/name/version).
**Exit:** the four providers open their TUI in the right cwd; a fake `agent` binary without "cursor" in `--version` is reported as `Rejected`; a GUI launched from Finder detects `claude` installed through a global npm.

## Phase 6 — Git worktrees
Discovery of existing worktrees; a creation dialog; GitService create/list/remove; a Project → Workspace → Sessions sidebar; "+ Agent" from a worktree; deletion safety.
**Exit:** Scenario C; deleting a worktree with changes without `force` returns `PreconditionFailed` with the list of checks.

## Phase 7 — Session graph foundation
Persisted parent/root; roles; `CreateChildSession` with `SameWorkspace`/`ExistingWorkspace`; a nested sidebar; the `ContextEnvelope` table and request; re-parenting on close.
**Exit:** Scenario E; closing a parent with two children leaves the children as roots; trying a parent that creates a cycle returns `InvalidRequest`.

## Phase 8 — Orca-style UX
A command palette (Add Project, New Terminal, New Agent, New Worktree, Focus Session, Close/Restart Session, Toggle Sidebar, Stop Daemon); session status with only the five states (no "waiting for input" heuristics); context menus; layout restore; an activity indicator; a notification when an agent exits; focus rings; OSC titles.
**Exit:** an external user uses the app for a working day without touching any configuration; the layout is restored after a restart.

## Phase 9 — Reliability and release
Crash boundaries per session task; an audit of every bounded queue; stress (`yes`, `cat` of 1 GB, `pnpm install`, 20 agents); IME on macOS and Linux; a clean shutdown; macOS packaging (bundle, signing, notarization, arm64; universal depending on distribution) and Linux (AppImage + deb; validate Wayland and X11; document runtime deps); `cargo deny`.
**Exit:** the §20 budgets measured and met on the reference hardware.

---

# 19. MVP acceptance criteria

**A — First use:** install, open, Add Project, choose a repo, create a Claude session, interact. Without manually configuring the daemon.

**B — Multiple agents:** Claude, Codex, OpenCode and Cursor coexist in four sessions of the same workspace; switching between them with `Cmd+Shift+]` takes < 100 perceptible ms.

**C — Worktree:** create `feature/auth`; launch Codex and a shell there; `pwd` in both returns the worktree's path; the main does not change branch.

**D — GUI restart:** start an agent; close the GUI; the agent keeps going (verifiable with `ps`); reopen; the sidebar shows the session; the terminal shows the current state with no lost or duplicated lines.

**E — Child session:** create A; New child session → B with the `Planner` role; restart the GUI **and** the daemon; B is still nested under A (both `Orphaned`) and B's `RestartSession` keeps the parent.

**F — Provider not installed:** without `opencode` on the PATH the picker shows "Not found"; the app does not fail; the rest work; "Set path…" to a valid binary turns it Installed without restarting.

**G — Daemon restart (new):** kill the daemon with `kill -9`; the GUI shows "Disconnected" and a "Restart daemon" button; on restart, every previous session appears `Orphaned` and none as Running.

**H — Backpressure (new):** with the GUI paused under a debugger and `yes` running for 30 s, the daemon's memory does not grow more than 50 MB and when it resumes the GUI receives a `TerminalResync`.

---

# 20. Performance budgets (reference hardware: MacBook Air M2 16 GB; ThinkPad X1 i7 / Intel iGPU, Fedora Wayland)

| Metric | Target |
|--------|--------|
| Frame time during normal interaction | ≤ 16 ms p95 |
| Key → echo latency | ≤ 33 ms p95 |
| Idle daemon (10 terminals with no output) | < 0.5 % CPU |
| Idle GUI | < 1 % CPU, 0 frames/s when nothing changes |
| `yes` throughput without freezing the UI | the UI responds while the engine processes ≥ 50 MB/s |
| Reattach to a terminal with 10k scrollback | < 200 ms to the first frame |
| Daemon memory per terminal (10k scrollback) | < 15 MB |
| Deltas per second to one subscriber | ≤ 125 |
| Cold start of GUI + daemon | < 1 s to an interactive window |

Principles: batched output (never one message per byte), no polling for global state (`try_wait` through SIGCHLD/`pidfd`/kqueue), memory bounded by the scrollback, partial invalidation of the view tree.

---

# 21. Testing

**Unit:** provider detection with fake binaries (an unrelated `agent` included); parsing `env -0` with multiline values; parsing `git status --porcelain=v2` and `git worktree list --porcelain`; the worktree slug and its collisions; protocol encode/decode with `non_exhaustive`; graph validation (cycles, depth, cross-project); re-parenting; `SessionState` transitions (the invalid ones fail); input mapping per `TermModes`; delta coalescing and the resync policy.

**Daemon integration:** a real daemon in the test process with a temporary socket; `cat`, `echo`, `sleep`, `bash`, an ANSI script and a continuous-output script as fixtures. Tests: spawn, I/O, resize, kill (with a `sleep` grandchild that must die), reconnect with grid verification, 20 concurrent sessions, a full queue → resync, `StopDaemon`.

**Git:** real temporary repos (`git init`, commits, branches, worktrees, changes, remove). Do not mock Git.

**Terminal golden:** ANSI input → the expected `TerminalSnapshot` with `insta`: colours, cursor, clear, alt buffer, Unicode/wide chars, resize with reflow.

**the shell:** UI tests for focus, actions, sidebar selection, the command palette, navigation. Headless Linux CI.

**Determinism:** the terminal tests use a fake `PtyBackend` (`fake_pty`) with predefined bytes; the integration ones use real PTYs.

---

# 22. Observability

`tracing` spans: `project.add`, `worktree.create`, `worktree.remove`, `session.create`, `session.kill`, `terminal.spawn`, `terminal.attach`, `agent.detect`, `agent.spawn`, `ipc.request` (with the `request_id` and the variant), `env.resolve`. `session_id`, `workspace_id`, `project_id` fields where applicable. Never terminal content nor environment values.

A `{app}-daemon stats` command and a dev screen (`Cmd+Shift+D` in debug): uptime, sessions per state, PTY bytes/s, IPC bytes/s, attached terminals, queue depth per client, resyncs emitted.

---

# 23. Security

Local-only IPC; a `0600` socket in a `0700` directory; no TCP; no content sent to services of ours; no API keys stored (the CLIs' authentication is inherited); logs without environment values; confirmation on destructive actions; the cwd always visible; OSC 52 disabled; `GIT_TERMINAL_PROMPT=0`. The daemon rejects paths with non-canonicalisable `..` components and paths outside the home unless explicitly confirmed in `AddProject`.

Future: an explicit and auditable `ContextEnvelope`; no agent reads another's transcripts automatically.

---

# 24. Agent evolution

1. **Terminal agents** (MVP): PTY + a CLI TUI.
2. **Metadata/adapters:** resume, an initial prompt, a provider session ID, completion detection when it is stable. The first real use of the `AgentAdapter` trait.
3. **ACP:** a second `AcpAgent` mode alongside `TerminalAgent`, without replacing it.
4. **Orchestration:** the daemon exposes `spawn_child`, `send_context`, `wait_for_session`, `collect_artifact` through the UI, our own CLI, MCP or agent tools.

# 25–26. Future orchestrator and conceptual API

Unchanged from v1: the system only needs the `Session`, `Context`, `Artifact`, `Workspace`, `Relation`, `Status` primitives, which exist from V1. `SessionOrchestrator { spawn_child, send_context, wait_for_exit }` is a design direction, not a frozen API.

# 27. What NOT to do in the MVP

Do not parse TUIs by scraping to infer "waiting for approval"; no per-provider branching outside `agents/`; no Git of our own; no plugin system; no networked daemon; no infinite scrollback; do not solve orchestration before `spawn PTY → render → input → persist → reconnect` is impeccable; **do not add a second emulator in the GUI** (ADR-011); **do not embed Zed's GPL editor nor build an IDE** (ADR-012: a minimal editor through the UI kit + `fs-service`).

# 28. Technical priority

```text
1. Terminal correctness   2. Daemon lifecycle   3. Reattach/snapshot   4. Project persistence
5. Multiple sessions      6. Agent launching    7. Worktrees           8. UI polish
9. Session graph         10. Orchestration
```

This order now matches the §18 phases.

# 29. Definition of Done per component

**Terminal:** `vim`, `less`, `htop` and the four TUIs; it does not lose input; resize with reflow; copy/paste; wide chars; restore after a GUI restart; resync under backpressure.
**Project:** a canonical path; persistent; Git detected; duplicate-safe; removal with the three policies.
**Agent:** detected with verification; a real executable; the override works; the right cwd; a visible exit code.
**Worktree:** create (a new and an existing branch); list (external ones included); launch; safe removal with checks.
**Session:** five states with valid transitions; Close/Kill/Restart; children and re-parenting.
**Daemon:** a singleton by flock; reconnect; no session death when closing the GUI; `Orphaned` after a restart; a clean shutdown.

# 30. Minimum documentation

`README.md` (what it is, platforms, build, status); `docs/architecture.md` (GUI vs daemon, the domain, the graph, ADR-011); `docs/protocol.md` (version, framing, requests, events, attach/seq/resync); `docs/terminal.md` (PTY, engine, deltas, renderer, scrollback); `docs/agents.md` (descriptor, builtins, detection, override, how to add one); `docs/worktrees.md` (location, slug, create, remove, safety); `docs/development.md` (how to run the GUI and the daemon separately, `dump --json`, stats).

# 31. Tentative dependencies (pinned after Phase 0)

```text
UI: Tauri 2 + Solid
Async: tokio (daemon), async-channel or flume (the GUI bridge)
Serialization: serde, rmp-serde
IDs/time: uuid (v7), time
Logging: tracing, tracing-subscriber, tracing-appender
DB: rusqlite (bundled), rusqlite_migration
PTY: portable-pty (to evaluate), nix/rustix as the alternative
Terminal: alacritty_terminal, vte
Strings: compact_str
Paths: directories
Errors: thiserror in the crates; anyhow only in the binaries
Config: toml, serde
CLI: clap
Test: tempfile, insta
Policies: cargo-deny
```

# 32. Risks

| Risk | Mitigation |
|------|------------|
| Tauri / Solid | keep the WebView shell thin; theme from tokens.ts |
| Terminal/rendering more expensive than expected | Phase 0 with a gate; `alacritty_terminal`; an engine abstraction; study Zed/Arbor |
| Linux (Wayland/X11, clipboard, fonts, IME) | Linux in the spike with X11 required; a bundled font; IME as a release gate |
| GUI vs terminal environment | `ShellEnvironmentService` with sentinels, a timeout and a fallback from Phase 1 |
| A provider changes its name/flags | ordered candidates + a version probe + a manual override |
| Input latency with ADR-011 | measured in 0.5; predictive local echo as the fallback |
| The protocol changes fast | an integer version; `non_exhaustive`; exact equality in the MVP |
| Licences | `cargo deny`; do not copy GPL crates |
| `portable-pty` insufficient | explicit criteria in §11.2 and a plan B with `nix` |

# 33. Milestones

```text
M0  Native shell        Tauri + Solid layout on macOS/Linux (Phase 0–1)
M1  Persistent terminal PTY + daemon + view + reattach + resync (Phase 2)
M2  Project manager     projects + multiple shells + tabs/splits (Phase 3–4)
M3  Agent manager       four providers with verified detection (Phase 5)
M4  Worktree workflows  create/list/remove + isolated agents (Phase 6)
M5  Product UX          palette, menus, layout, notifications (Phase 8)
M6  Session graph       manual children + roles + ContextEnvelope + scenarios A–H (Phase 7, 9)
```

**M6 = MVP.**

# 34. Post-MVP roadmap

1. Structured agent lifecycle (session IDs, resume, initial prompt, completion). 2. Roles and templates. 3. Manual context passing ("Send context to…"). 4. Our own CLI (`{app} session list/spawn`, `{app} context send`) using `{APP}_SESSION_ID`. 5. MCP/orchestration tools. 6. ACP. 7. Basic diff/review. 8. A remote daemon (the same protocol over a secure channel).

# 35–36. Final flow and recommendation

No substantial changes: the primitive is **a persistent session running in a workspace, optionally associated with an agent and related to other sessions**. If Session + Workspace + PTY + Graph are well resolved, orchestration is an evolution; if it is modelled as "four buttons that open CLIs", it will force a rewrite of the core.

# 37. References

Orca https://github.com/stablyai/orca · T3 Code https://github.com/pingdotgg/t3code (docs/internals/overview.md, docs/reference/encyclopedia.md) · Herd https://github.com/allenan/herd · Arbor https://github.com/penso/arbor · 

# 38. Startup checklist (the real execution order)

- [ ] The Cargo workspace, `rust-toolchain.toml`, `deny.toml`, CI macOS + headless Linux
- [ ] Scaffold Tauri + Solid shell
- [ ] The `domain`, `protocol`, `terminal-core`, `daemon`, `client`, `app` crates
- [ ] Typed UUIDv7 IDs; `Timestamp`
- [ ] Paths + socket length validation
- [ ] Daemon: flock + socket + Hello/HelloAck/HelloReject
- [ ] GUI: the IPC thread + `connect_or_spawn_daemon` + retries
- [ ] `config.toml`
- [ ] The Tauri layout spike (macOS, Wayland, X11)
- [ ] The PTY spike with the §11.2 criteria
- [ ] `alacritty_terminal` behind `TerminalEngine` with damage
- [ ] `TerminalSnapshot`/`TerminalDelta` + seq + resync + 8 ms coalescing
- [ ] TerminalView with `CellGrid`, a bundled font
- [ ] Input mapping + tests
- [ ] Validate `vim`/`htop`/the four TUIs; measure the latency
- [ ] GUI restart/reattach verified against the daemon's grid
- [ ] SQLite + migrations + `Orphaned` reconciliation
- [ ] Add/Remove Project (three policies) + Git discovery
- [ ] SessionService: states, Close/Kill/Restart, re-parenting
- [ ] Multiple sessions, tabs, splits
- [ ] `ShellEnvironmentService` with sentinels/timeout/fallback
- [ ] AgentRegistry + verified detection + override
- [ ] Integrate Claude, Codex, OpenCode, Cursor
- [ ] Worktrees: slug, create (new/existing), list, remove with checks
- [ ] A Project → Workspace → Session sidebar with nesting
- [ ] `CreateChildSession` + roles + the `context_envelopes` table
- [ ] The command palette, context menus, a persisted layout, the activity indicator, notifications
- [ ] Stress tests (§21) and budgets (§20)
- [ ] IME macOS/Linux
- [ ] Packaging macOS (sign/notarize) and Linux (AppImage/deb)
- [ ] `docs/*.md`

---

# 39. Glossary and resolved ambiguities

**Glossary**

| Term | Definition |
|------|------------|
| Project | A directory added by the user; optionally a Git repo |
| Workspace | The effective directory where sessions run: the main checkout or a worktree |
| Session | A persistent unit of work in the domain; Shell or Agent; a node of the graph |
| TerminalRuntime | The PTY + process + engine alive in the daemon; ephemeral; identified by a `TerminalId` |
| Attach | A client's subscription to a terminal's deltas |
| Snapshot | The grid's complete state at one `seq` |
| Delta | The rows changed between two `seq` |
| Resync | A snapshot forced by a full queue |
| Orphaned | A session that was Running when the daemon restarted |
| Managed worktree | A worktree created by the app in its data directory |
| Descriptor / Adapter | The declarative definition of a provider / special behaviour in code |
