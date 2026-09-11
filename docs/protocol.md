# IPC protocol

How `forge` (GUI) and any other client talk to `forge-daemon`. The protocol is
defined in `crates/protocol` and is transport-agnostic; the transport itself is
a Unix domain socket (ADR-004). The GUI-side implementation is
`crates/client` (`Client` + `Store`).

Plan references: §9.2, §10. `PROTOCOL_VERSION = 18`.

## Transport & framing

- **Socket:** `$TMPDIR/forge/daemon.sock` on macOS, `$XDG_RUNTIME_DIR/forge/`
  on Linux, mode `0600`. The full path must be under 100 bytes (`sun_path`
  limit); otherwise the daemon falls back to `/tmp/forge-$UID/` and says so —
  it never silently truncates.
- **Frame:** `u32` big-endian length + MessagePack payload (`rmp-serde`, named
  fields), in `crates/protocol/src/framing.rs`.
  `protocol::framing::MAX_FRAME_SIZE = 16 * 1024 * 1024`; a larger declared
  length is fatal (`ProtocolCodecError::FrameTooLarge` → close the connection),
  never a truncated read.
- `FrameDecoder` is the incremental decoder both sides use; `to_json_string`
  renders any message as JSON for debugging (`forge-daemon dump --json`, not
  yet wired).

## Handshake (§9.2)

```
client → Hello { protocol_version, client_version, client_kind }
daemon → HelloAck { protocol_version, daemon_version, instance_id, started_at }
       | HelloReject { daemon_protocol_version, reason }
```

The MVP requires `protocol_version` **equality**; N/N-1 compatibility is
deferred. `instance_id` comes from `daemon.lock` and lets a client notice a
daemon restart. `ClientKind` is `Gui` (the debug CLI also uses it) or `Unknown`.

## Message model (§10.1)

```
ClientMessage::Request  { request_id: u64, body: Request }
DaemonMessage::Response { request_id: u64, body: Result<Response, ProtocolError> }
DaemonMessage::Event    (DaemonEvent)
```

- Requests are correlated by `request_id`; the client may pipeline.
- **Mutations answer `Ack`** and the resulting domain object arrives as a
  broadcast `DaemonEvent` to *every* client (including the caller). Clients
  therefore have one code path for "state changed", not two.
- Domain events are low-volume and broadcast. Terminal deltas go only to
  clients that attached to that terminal; others get a coalesced
  `TerminalActivity` (≤1/s per terminal) for unread badges.

## Requests (§10.2)

| Group | Request | Answer / side effect |
|-------|---------|----------------------|
| Global | `GetSnapshot` | `Response::Snapshot { project_groups, projects, workspaces, sessions, providers, agent_profiles, app_state, external_agents, pull_requests, jobs }` — the full initial state. `pull_requests` is served from the daemon's cache and is **never** a network read: a snapshot is the first thing a reconnecting GUI asks for, and it must not wait on GitHub. |
| | `StopDaemon { kill_sessions }` | Refuses if sessions run and `kill_sessions = false`; else `DaemonShuttingDown` to all. |
| | `FactoryReset` | Stops every session and job, transactionally clears the Forge metadata database, force-removes worktrees created by Forge, and broadcasts `FactoryReset` so clients bootstrap again. Repository files, branches, commits, `config.toml`, logs, and repository-owned `harness/` files are retained. |
| | `GetAppState { key }` / `SetAppState { key, value }` | Opaque key/value for GUI layout etc. `GetAppState` answers `Response::AppState { value }`. |
| | `RefreshPullRequests` | `Ack` **as soon as the refresh starts**, then `PullRequestsUpdated` when it finishes. Resolves each project's default remote locally, then makes at most one `gh api graphql` call per eligible host. Coalescing is global — one refresh already covers every repository — and a result younger than 60 s is re-broadcast from the cache instead of re-queried. |
| Projects | `AddProject { path }` | `Ack`; `ProjectAdded` (+ `WorkspaceCreated` for the main workspace). Git root detected via `rev-parse`. |
| | `AddProjectToGroup { path, project_group_id }` | Adds a new project directly to an organizational workspace. |
| | `CreateProjectGroup { name }` / `RenameProjectGroup { project_group_id, name }` | Creates or renames an organizational workspace; no filesystem or Git operation. |
| | `MoveProject { project_id, project_group_id }` | Changes only the project's organizational parent; `None` means General. |
| | `RemoveProjectGroup { project_group_id }` | Deletes only the grouping; its projects move to General and all directories/worktrees/sessions remain untouched. |
| | `RemoveProject { project_id, policy }` | See policies below. `ProjectRemoved`. |
| | `RefreshProject { project_id }` | Re-detects the git root, reconciles the worktrees on disk, and re-reads each workspace's branch and status; `ProjectUpdated`, plus `WorkspaceCreated`/`WorkspaceRemoved`/`WorkspaceUpdated` for what changed. |
| | `RenameProject { project_id, name }` | `ProjectUpdated`. |
| | `SetProjectIcon { project_id, icon }` | `ProjectUpdated`. `None` (or a blank string) clears it and the UI falls back to the project's initials; anything that is not a single drawable mark is refused with `InvalidRequest` rather than quietly cleared. |
| Workspaces | `ListWorkspaces { project_id }` | `Response::Workspaces`. |
| | `CreateWorktree { project_id, branch, base, name }` | `WorkspaceCreated`; see [worktrees.md](./worktrees.md). |
| | `RemoveWorktree { workspace_id, force }` | `WorkspaceRemoved`; pre-checks unless `force`. |
| | `RenameWorkspace { workspace_id, display_name }` | `WorkspaceUpdated`. `None` clears the human label so the rail falls back to the branch. |
| | `RefreshWorkspaceStatus { workspace_id }` | `WorkspaceUpdated` carrying branch + `WorkspaceStatus { dirty, ahead, behind }`. Throttled to one `git status` every 2 s per workspace (ADR-008). |
| Branches | `ListBranches { project_id }` | `Response::Branches { branches, remotes, default_branch }`. A **local** ref read — never touches the network, so it is safe on a thread that also carries keystrokes. Each `BranchRef` says which workspace already has it checked out. |
| | `FetchRemote { project_id, remote }` | `Ack` **as soon as the fetch starts**, then `RemoteRefsUpdated` when it finishes. One of the asynchronous requests (with `CreatePullRequest` and `RefreshPullRequests`): a fetch can take minutes and the GUI's command channel also carries terminal input. A fetch already in flight for that project is coalesced, not queued. `remote: None` = `origin`, falling back to the only remote configured; `GitError` when there is none. |
| | `GetWorkspaceDiff { workspace_id, context_lines }` | `Response::WorkspaceDiff(WorkspaceDiff)` — the checkout's uncommitted changes, one unified patch per file (§16.7). Local and **synchronous**, like `ListBranches`: `git diff` opens no socket, so there is nothing to ack early and report through an event. Not broadcast either: a diff is a view one client asked for, not shared state. `context_lines: None` takes the service default (12, wider than git's 3 — this feeds a window, not a pager). |
| | `ListFiles { workspace_id }` | `Response::FileTree(FileTree)` — tracked and untracked-but-not-ignored paths (ADR-012). Local and **synchronous** like `GetWorkspaceDiff`. Paths are relative and stay inside the checkout. |
| | `ReadFile { workspace_id, path }` | `Response::FileContents(FileContents)` — text plus a `revision` the next write must present. Refuses binaries and oversize files without truncating. |
| | `WriteFile { workspace_id, path, text, expected_revision }` | `Ack`. `PreconditionFailed` when the on-disk content no longer matches the revision (an agent wrote the same path); the GUI re-reads with `ReadFile`. |
| | `SearchFiles { workspace_id, query, kind, limit }` | `Response::SearchResults(SearchResults)` — fuzzy name match, `git grep` content search, or `Definition`: a `git grep -w -F` for the bare word kept only where the line declares it. The answer echoes `query`, which is what tells a client whose lookup it is. |
| Sessions | `CreateShellSession { workspace_id, parent, role }` | `SessionCreated` (state `Starting`, then `SessionUpdated` → `Running`). |
| | `CreateAgentSession { workspace_id, provider_id, profile_id, parent, role, resume, initial_prompt, read_only }` | Same; `ProviderNotInstalled` if detection failed. `profile_id` applies a launch profile (§13.4): `NotFound` if it is gone, `InvalidRequest` if it belongs to another provider. `resume` is a provider session id to re-enter instead of starting fresh (§13.5): `InvalidRequest` when that provider declares no way to resume. `initial_prompt` is a one-shot task string when the provider supports it (harness orchestrator, PR compose, PR review). `read_only` launches the provider in its own read-only mode (§16.9), which is what an automatic pull-request review runs in: `InvalidRequest` when the provider declares none, and unlike the prompt it is re-applied on a restart. A restart of the session re-enters the same conversation. |
| | `CreateChildSession { parent_session_id, kind, provider_id, profile_id, role, workspace_policy, initial_prompt }` | Child under a parent; workspace chosen by `ChildWorkspacePolicy`. `SameWorkspace` and `ExistingWorkspace` work; `NewManagedWorktree` is rejected with `InvalidRequest` (not wired yet). `initial_prompt` seeds harness implementer/reviewer children. |
| | `KillSession { session_id }` | Signals the process group; `SessionUpdated` → `Exited` when the PTY closes. |
| | `CloseSession { session_id }` | Removes the session. **Rejected while active** (`PreconditionFailed`). Children re-parent to the grandparent (`SessionUpdated` for each), then `SessionRemoved { session_id }` is broadcast. |
| | `RestartSession { session_id }` | Only from `Exited`/`Failed`/`Orphaned`; new `TerminalId`. |
| | `RenameSession { session_id, title }` | `None` clears the user title. |
| | `SetSessionRole { session_id, role }` | `SessionUpdated`. |
| | `CreateContextEnvelope { envelope }` | Persists it. |
| | `SendContext { source_session_id, target_session_id?, spawn?, summary?, instructions?, include_transcript, max_transcript_bytes? }` | Exactly one of `target_session_id` or `spawn`. Persists an envelope; pastes a framed block into a live target's PTY, or answers `SessionCreated` when spawning a child that starts with that text as `initial_prompt`. How-to: [session-context.md](./session-context.md). |
| | `ListContextEnvelopes { session_id }` | `Response::ContextEnvelopes` — envelopes where the session is source or target. |
| | `GetSessionTranscript { session_id, max_lines?, max_bytes? }` | Plain text off the session's terminal (cite / handoff capture). |
| Terminals | `AttachTerminal { terminal_id, size }` | `Response::AttachAck { snapshot }`; subscribes to deltas. `size` is adopted **only when this is the first subscriber** — attaching to a terminal someone else is already watching never resizes it under them. Use `ResizeTerminal` to change a shared terminal. |
| | `DetachTerminal { terminal_id }` | Unsubscribe. |
| | `WriteTerminalInput { terminal_id, bytes }` | Raw bytes to the PTY master. Key→bytes mapping is done client-side through the pure `terminal-input` crate, re-exported by `client`. |
| | `ResizeTerminal { terminal_id, size }` | Last writer wins. |
| | `FetchScrollback { terminal_id, from_line, count }` | `Response::ScrollbackRows`. `from_line` is absolute (0 = oldest). |
| | `SendSignal { session_id, signal }` | `SigInt`, `SigTerm`, `SigHup`, `SigKill` to the process group. |
| Agents | `ListAgentProviders` | `Response::Providers(Vec<ProviderInfo { descriptor, detection }>)`. |
| | `RefreshAgentDetection { provider_id }` | `Some(id)` re-probes exactly that provider (`NotFound` for an unknown id) without disturbing the other cached results; `None` re-detects all. `AgentDetectionChanged`. |
| | `SetProviderExecutable { provider_id, path }` | Override persisted; `None` clears. |
| | `SaveAgentProfile { profile }` | Create or replace a launch profile (§13.4); one upsert, like `CreateContextEnvelope`, so the client builds the whole object including its id. Validated here: `Conflict` for a duplicate name, `InvalidRequest` for a bad or reserved variable, `ProviderNotInstalled` when its own executable fails the version probe. `AgentProfilesChanged`. |
| | `RemoveAgentProfile { profile_id }` | Deletes it; sessions it already started keep running. `AgentProfilesChanged`. |
| Shared files | `DetectShareCandidates { project_id }` | `Response::ShareCandidates` — the project's ignored paths, classified, with the strategy Forge would propose. One `git status --ignored=matching` plus a bounded walk: local and **synchronous** like `ListBranches`, and `truncated` says the list is a floor rather than the whole tree. |
| | `SetProjectShares { project_id, rules }` | Replaces the project's whole rule set (§14.2). The set, not a row: adding, reordering, enabling and re-pointing a strategy are one edit of one list. `InvalidRequest` for a path that is absolute, contains `..`, or names the git directory. `ProjectSharesChanged`. |
| | `RemoveShareRule { project_id, rule_id, cleanup }` | Drops one rule and says what happens to the files it already wrote: `Leave`, `RemoveInjected` or `Materialize`. Answers `Response::SharePlan` with what the cleanup did per workspace. Its own request because it is the only edit that can delete a file. `ProjectSharesChanged`. |
| | `PreviewShares { workspace_id }` | `Response::SharePlan` — what applying would do. Writes nothing. |
| | `GetShareStatus { workspace_id }` | `Response::ShareStatus` — per rule: `Applied`, `Missing`, `Severed` (a link a tool replaced with a real file), `Diverged`, `Skipped` or `Failed`. One `lstat` per rule, no subprocess. |
| | `ApplyShares { workspace_id, only }` | Acks when the work **starts**; the outcome arrives as `SharesApplied`. Coalesced per workspace, like `FetchRemote` — a `Run` rule is a subprocess and the GUI channel that carries this also carries every keystroke. `only: Some(ids)` is the user asking for those rules by name, which is the only case where something already on disk may be backed up and replaced. |
| | `AdoptIntoShareStore { project_id, path }` | Moves a real file into `<git common dir>/forge/shared` and leaves a symlink, backing the original up first. Explicit because it is the only operation that changes the primary checkout. |
| | `MaterializeFromShareStore { project_id, path, workspace_id }` | The inverse: writes the store file back as a real file. `workspace_id: None` does it for every workspace of the project. |
| | `ListProviderUsage` | `Response::ProviderUsage(Vec<ProviderUsage>)` — each *account's* remaining allowance, read from its own endpoint or CLI (§16.2): the default login of every provider that declares a source, plus one reading per launch profile that moved the config directory, each stamped with its `profile_id`. Re-read rather than served from the cache, and also broadcast as `ProviderUsageChanged` so every client sees the same reading. A provider that declares no source is simply absent. |
| | `GetUsageAnalytics { window_days }` | `Response::UsageAnalytics(Box<UsageAnalytics>)` — what the agents on this machine actually spent, counted off their own transcripts (§16.2). Local and **synchronous**, like `GetWorkspaceDiff`, and a read in the same sense: nothing is broadcast and nothing is stored. The daemon caches the scan for 60 s per window, because it is line-by-line IO over every recent transcript. `window_days: None` takes the reader's default (30) and an over-large value is clamped, never refused. |
| Harness | `ListHarnessFeatures { project_id }` | `Response::HarnessFeatureList` — reads `harness/features.json` under the project root. Local and **synchronous**; no broadcast. |
| | `GetHarnessFeature { project_id, feature_id }` | `Response::HarnessFeature`. |
| | `GetHarnessTimeline { project_id, feature_id }` | `Response::HarnessTimeline` — tail of `harness/progress/events_<id>.jsonl`. |
| | `ReadHarnessArtifact { project_id, feature_id, kind }` | `Response::HarnessArtifact` — gate, context, impl, review, or spec markdown. |
| | `RegisterHarnessFeature { project_id, workspace_id, spec_raw, title }` | `Response::HarnessFeature` — appends a row and writes `spec_raw`. `workspace_id` is the checkout the feature will be implemented in: the state file is one per *repository*, so this is what lets two worktrees of a project each run their own feature. `Conflict` when that checkout already holds an open one, `InvalidRequest` when the workspace belongs to another project, and `None` leaves the feature unattached like one registered from the CLI. |
| | `RegisterHarnessFromIssue { project_id, workspace_id, issue }` | `Response::HarnessFeature` — seeds from a GitHub issue via `gh`; `workspace_id` as above. |
| | `HarnessAdvance { project_id, feature_id, action }` | `Response::HarnessFeature` — human gate (`ApproveSpec`, `ReviseSpec`, `Block`) or cycle (`StartImplement`, `StartReview`). |
| | `LinkHarnessSession { project_id, feature_id, session_id }` | `Ack` — persists `orchestrator_session_id` on the feature row. |
| | `ValidateHarness { project_id }` | `Response::HarnessValidate` — runs the harness validator (`scripts/harness validate` logic). |
| | `RunHarnessStep { project_id, feature_id, step }` | `Response::Job` — runs one step (`Spec`/`Implement`/`Review`) as a headless job. The daemon writes the step's `*_started` event, sets the status and starts the agent; when that job **exits** it records the outcome and starts whatever comes next, stopping at the human gate. A client therefore asks once at the start and once after an approval — the rest arrives as `JobUpdated` and `HarnessFeatureChanged`. `InvalidRequest` when the feature names no checkout or no headless-capable agent is configured. |
| Jobs | `StartJob { request }` | `Response::Job` — a **headless** run of one provider (`claude -p`, `codex exec`): no PTY, one prompt, an exit code. Answers as soon as the job is *accepted*, which may be before it starts — jobs queue behind a concurrency ceiling, because every run spends the same account rate limit as the interactive sessions beside it. `InvalidRequest` when the provider declares no headless mode, or when `resume_from` is set and it cannot resume one. The same binary detection verified, started from the same login-shell environment, so it reads the same subscription login; there is no API-key path. |
| | `CancelJob { job_id }` | `Ack`. Kills the process **group**, so the CLI's own children go with it; a queued job is dropped and a finished one is left alone. |
| | `ListJobs` | `Response::Jobs` — every job this daemon has run, oldest first. Runtime-only: a job is a process, and none survive a restart. |
| | `ReadJobLog { job_id, from_line }` | `Response::JobLog` — the provider's event stream from disk, in the same readable form the live `JobOutput` carries, so a step opened mid-run reads as one stream. `from_line` skips what a follower already has. The **verbatim** record is the file at `Job::log_path`, which the daemon never edits. |

Removed relative to the v1 plan: `FocusSession` (pure GUI state) and generic
`Subscribe`/`Unsubscribe` (attach *is* the subscription).

### `RemoveProjectPolicy`

| Policy | Sessions | Worktrees | Branches |
|--------|----------|-----------|----------|
| `KeepEverything` | Rejected if any session is active | untouched | untouched |
| `KillSessionsKeepWorktrees` | killed | untouched | untouched |
| `KillSessionsRemoveManagedWorktrees` | killed | only `managed_by_app` worktrees removed from disk | **never deleted** |

## Events (§10.3)

| Event | Notes |
|-------|-------|
| `ProjectAdded` / `ProjectUpdated` / `ProjectRemoved { project_id }` | Full object on add/update. |
| `ProjectGroupCreated` / `ProjectGroupUpdated` / `ProjectGroupRemoved { project_group_id }` | Organizational metadata only. |
| `WorkspaceCreated` / `WorkspaceUpdated` / `WorkspaceRemoved { workspace_id }` | |
| `SessionCreated` / `SessionUpdated` | The only session update path — state, title, role, parent, `terminal_id` all ride on it. |
| `SessionRemoved { session_id }` | After `CloseSession`, and once per removed session during `RemoveProject`. |
| `TerminalDelta { terminal_id, delta }` | Subscribers only. |
| `TerminalResync { terminal_id, snapshot }` | Subscriber fell behind; replace the grid. |
| `TerminalActivity { terminal_id }` | Non-subscribers, coalesced ≤1/s. |
| `TerminalBell { terminal_id }` | Broadcast to **all** clients, not only subscribers. |
| `RemoteRefsUpdated { project_id, remote, updated, error }` | A background `FetchRemote` finished. Carries the outcome, not the refs: clients re-ask `ListBranches` when they care, which keeps one source of truth. `error` holds git's own words (`Permission denied (publickey)`) on failure. |
| `PullRequestOpened { workspace_id, url, error }` | A background `CreatePullRequest` finished. `url` on success, `error` with the CLI's own words otherwise. A success also invalidates the pull-request cache and starts a refresh, so the new PR appears without waiting out the TTL. |
| `PullRequestsUpdated { state }` | A background `RefreshPullRequests` finished. Carries the **whole** `PullRequestState` — like usage and detection, a merge of two refreshes would leave a client with a list that never existed. `state.sources` says why a project contributed nothing (no repo, no remote, unsupported host, unparseable remote, failed); `state.failures` holds per-repository errors that did not sink the rest; `state.error` is set only when every queried host failed. |
| `FileChanged { workspace_id, path }` | A watched file changed on disk (ADR-012). Carries no content — the GUI re-reads with `ReadFile` and offers reload-or-keep when the buffer is dirty. |
| `HarnessFeatureChanged { project_id, feature }` | A feature row changed because a step finished. The daemon advances the cycle itself, so a client that asked for nothing still learns that a spec is ready for its gate, or that a feature is done. |
| `JobUpdated(Job)` | A headless run was accepted, started, or reached a final state; carries the whole row, like `SessionUpdated`. **This is the event a harness step waits on**: a job in a final state has finished, with an exit code that says how, and nobody had to read a terminal to find out. |
| `JobOutput { job_id, from_line, lines }` | Lines a running job wrote, coalesced on a ~120 ms interval and summarised for reading (a raw `stream-json` line is not something a person reads; the provider's own words stay in the log file). Broadcast to every client, like `TerminalActivity`: a job has no subscription because it has no grid to keep in sync, and its output is text a client either follows live or reads later with `ReadJobLog`. |
| `AgentDetectionChanged { results }` | After startup detection or a refresh. |
| `AgentProfilesChanged { profiles }` | The whole set after any profile write (§13.4) — like usage and detection, a merge would strand a deleted profile. |
| `ProjectSharesChanged { project_id, rules }` | One project's whole rule set after any write (§14.2), for the same reason as the profiles event. |
| `SharesApplied { workspace_id, trigger, actions, error }` | A provisioning run finished — after `CreateWorktree`, after a worktree was adopted, or after `ApplyShares`. `actions` is one line per rule (`verb`, `bytes`, `fallback`, `note`); `error` is set only when the run could not be attempted at all, because a worktree with three of four shares is still a usable worktree. |
| `DaemonNotice { level, message }` | `Warning`/`Error`/`Info`, e.g. login-shell env fallback. |
| `DaemonShuttingDown { reason }` | |

## Attach, sequences and resync (§10.5)

```
GUI                                DAEMON
 ├── AttachTerminal(size) ────────►│ adopts size only if first subscriber
 │◄── AttachAck(snapshot seq=N) ───┤ full grid + scrollback tail
 │◄── TerminalDelta(seq=N+1) ──────┤ damaged rows only
 │◄── TerminalDelta(seq=N+2) ──────┤
 │                                 │ subscriber queue full ⇒ drop, then:
 │◄── TerminalResync(snapshot) ────┤ one fresh snapshot, never a backlog
```

`seq` is per terminal and monotonic. The client discards `seq ≤ last`, applies
`seq == last + 1`, and on a gap re-attaches instead of guessing. Deltas reach
only the clients that attached; everyone else sees `TerminalActivity`. The
mechanics (8 ms coalescing, the 256-deep queue, `emit_seq`) are in
[terminal.md](./terminal.md).

## Errors

`ProtocolError { code, message, details }` with `ErrorCode`:
`InvalidRequest`, `NotFound`, `Conflict`, `PreconditionFailed`, `GitError`,
`SpawnError`, `IoError`, `ProviderNotInstalled`, `ProtocolViolation`,
`Internal`, `Unknown`.

`PreconditionFailed` is the "you must do something first" code: close an active
session, remove a worktree with running sessions / dirty tree / merge in
progress, remove a project with `KeepEverything` while sessions run.

## Forward compatibility

- Every protocol enum is `#[non_exhaustive]`.
- The enums that appear as *fields* rather than as message discriminants
  (`ErrorCode`, `ClientKind`, `Signal`, `NoticeLevel`) each carry an `Unknown`
  variant with `#[serde(other)]`, so a newer daemon naming a value an older
  client has never heard of does not break its decoder.

## The `client` crate

`client::Client` is deliberately **synchronous** (`std` Unix sockets, one
reader thread, `flume` channels — no tokio, no GUI toolkit):

- `Client::connect(socket_path, client_version)` performs the handshake and
  exposes `daemon_info()`. The `Hello` is built internally from
  `PROTOCOL_VERSION` and `ClientKind::Gui`; callers do not supply one.
- `request(body)` / `request_timeout(body, dur)` block until the matching
  response arrives. They must be bridged off the UI thread.
- `events()` returns an unbounded receiver the GUI must drain promptly.
- On EOF every pending request wakes with `ClientError::Disconnected`.
- Typed wrappers cover the requests the GUI makes by hand, including
  `set_app_state(key, value)` / `get_app_state(key) -> Option<String>`
  (`SetAppState`/`GetAppState`) and
  `set_provider_executable(provider_id, Option<PathBuf>)`
  (`SetProviderExecutable`, `None` clears the override). Ack-only requests
  return `()`; a non-`Ack` answer is `ClientError::UnexpectedResponse`.

`client::Store` is the passive replica: `apply_snapshot`, `apply_event`,
`attach_terminal`, `detach_terminal`, `merge_scrollback`, `app_state_value(key)`
for a persisted GUI preference from the last snapshot, and a `CellGrid` per
attached terminal. `EventOutcome`/`DeltaOutcome` tell the GUI when it must
re-attach (sequence gap) — see [terminal.md](./terminal.md).
