# IPC protocol

How `forge` (GUI) and any other client talk to `forge-daemon`. The protocol is
defined in `crates/protocol` and is transport-agnostic; the transport itself is
a Unix domain socket (ADR-004). The GUI-side implementation is
`crates/client` (`Client` + `Store`).

`PROTOCOL_VERSION = 27` (`protocol::PROTOCOL_VERSION` is the source).

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

## Tauri workbench envelope

The WebView's `send_workbench_command` is a separate host API, not an addition
to the daemon wire. It rejects when the worker is absent, closed or its bounded
queue is full. Accepted means enqueued, not completed.

- `create_path`, `rename_path`, `delete_path` carry `operation_id` and finish on
  `workbench:path_result { operation_id, workspace, kind, from, to, success,
  error, uncertain }`. Structured daemon refusals are definitive; transport
  failures are uncertain. Listing failures never complete mutations.
- `load_file_directory` carries `{ workspace, path, request_id, generation }`.
  `workbench:directory` echoes these plus `{ entries, truncated }`;
  `workbench:directory_failed` echoes them plus `message`.
- `load_file_tree` carries `{ workspace, request_id }`; its success echoes those
  plus `tree`, and failure echoes them plus `error`. The request identity rejects
  previous workspace/connection reads; invalidation during a read owes a follow-up.
- `watch_files` carries a generation echoed by `workbench:watch_ready` and
  `workbench:watch_failed`, so an old ACK cannot arm new interests.

These identifiers belong to the UI interaction/connection lifecycle. Daemon
request/reply IDs still correlate the actual filesystem operation. Unknown
write outcomes are reconciled by reading affected paths, never by replaying a
mutation automatically.

## Tauri targeted paste

The host's `runtime:connected` payload and cached `connect` answer include a
`connection_generation`, incremented for every successful host connection even
when the daemon instance and PTY identities survive. `send_runtime_command`
accepts `paste_target { session_id, terminal_id, connection_generation, text }`.
It rejects a disconnected/stale generation at enqueue; the runtime also drops
queued references from an older generation before execution. It refuses a
changed attachment, mismatched session-to-terminal mapping or ended session,
then uses that terminal replica's modes with `encode_paste`. No Enter is appended.
Queue acceptance is not execution confirmation; execution refusals use the
existing runtime notice. Neither rejection nor reconnect replays a paste.

The WebView constructs a shell-quoted absolute path from its known workspace
root, rejecting control characters. This host-local envelope adds no daemon
message: the encoded bytes travel through ordinary terminal input. Filesystem
drops use the correlated workbench `rename_path` operation above.

## Handshake (§9.2)

```
client → Hello { protocol_version, client_version, client_kind }
daemon → HelloAck { protocol_version, daemon_version, instance_id, started_at, editor_surface }
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
- **Mutations normally answer `Ack`** and the resulting domain object arrives as a
  broadcast `DaemonEvent` to *every* client (including the caller). Clients
  therefore have one code path for "state changed", not two. Session creation
  also returns `Response::SessionCreated { session_id, terminal_id }`; use those
  IDs directly rather than reloading a snapshot to guess which session is new.
- Domain events are low-volume and broadcast. Terminal deltas go only to
  clients that attached to that terminal; others get a coalesced
  `TerminalActivity` (≤1/s per terminal) for unread badges.

## Requests (§10.2)

| Group | Request | Answer / side effect |
|-------|---------|----------------------|
| Global | `GetSnapshot` | `Response::Snapshot { project_groups, projects, workspaces, sessions, providers, agent_profiles, worktree_shares, worktree_ignores, app_state, external_agents, pull_requests, usage }` — the full initial state. `pull_requests` is served from the daemon's cache and is **never** a network read: a snapshot is the first thing a reconnecting GUI asks for, and it must not wait on GitHub. |
| | `StopDaemon { kill_sessions }` | Refuses if sessions run and `kill_sessions = false`; else `DaemonShuttingDown` to all. |
| | `FactoryReset` | Stops every session, transactionally clears the Forge metadata database, force-removes worktrees created by Forge, and broadcasts `FactoryReset` so clients bootstrap again. Repository files, branches, commits, `config.toml` and logs are retained. |
| | `GetAppState { key }` / `SetAppState { key, value }` | Opaque key/value for GUI layout etc. `GetAppState` answers `Response::AppState { value }`. |
| | `RefreshPullRequests` | `Ack` **as soon as the refresh starts**, then `PullRequestsUpdated` when it finishes. Resolves each project's default remote locally, then makes at most one `gh api graphql` call per eligible host. Coalescing is global — one refresh already covers every repository — and a result younger than 60 s is re-broadcast from the cache instead of re-queried. |
| | `GetStats` | `Response::DaemonStats` — session counts, open terminals, connected clients, uptime. Local and **synchronous**, like `GetWorkspaceDiff`. `forge-daemon stats` is this request; `dump --json` is still unwired. |
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
| | `RefreshWorkspaceStatus { workspace_id }` | `WorkspaceUpdated` carrying branch + `WorkspaceStatus { dirty, head, ahead, behind }` — `head` is the commit oid, the signal the GUI re-reads the file tree and diff on. Throttled to one `git status` every 2 s per workspace (ADR-008). |
| Branches | `ListBranches { project_id }` | `Response::Branches { branches, remotes, default_branch }`. A **local**, synchronous ref read; the GUI runs it on the workbench worker because Git can block terminal input even without network access. Each `BranchRef` says which workspace already has it checked out. |
| | `FetchRemote { project_id, remote }` | `Ack` **as soon as the fetch starts**, then `RemoteRefsUpdated` when it finishes. One of the asynchronous requests (with `CreatePullRequest` and `RefreshPullRequests`): a fetch can take minutes and the GUI's command channel also carries terminal input. A fetch already in flight for that project is coalesced, not queued. `remote: None` = `origin`, falling back to the only remote configured; `GitError` when there is none. |
| | `GetWorkspaceDiff { workspace_id, context_lines }` | `Response::WorkspaceDiff(WorkspaceDiff)` — the checkout's uncommitted changes, one unified patch per file (§16.7). Local and **synchronous**, like `ListBranches`: `git diff` opens no socket, so there is nothing to ack early and report through an event. Not broadcast either: a diff is a view one client asked for, not shared state. `context_lines: None` takes the service default (12, wider than git's 3 — this feeds a window, not a pager). |
| | `ListFiles { workspace_id }` | `Response::FileTree(FileTree)` — tracked and untracked-but-not-ignored paths, plus opaque ignored directories (ADR-012). Language dependency directories (`node_modules`, `vendor`, …) are omitted. Local and **synchronous** like `GetWorkspaceDiff`. Paths are relative and stay inside the checkout. |
| | `ListDirectory { workspace_id, path }` | `Response::DirectoryListing { path, entries, truncated }` — immediate real disk children; `path: ""` reads root. Includes empty/hidden/ignored directories; `.git` and dependency directories are omitted. Optional `FileEntry.symlink` distinguishes internal file/directory links from external, broken and unavailable targets. Internal aliases retain their logical paths. Bounded to 2000 entries and 1 MiB per answer, with 4096-byte paths; partial results are explicit and read errors propagate. Local and **synchronous** like `ListFiles`. |
| | `ReadFile { workspace_id, path }` | `Response::FileContents(FileContents)` — text plus a `revision` the next write must present. Refuses binaries and oversize files without truncating. |
| | `ReadImage { workspace_id, path }` | `Response::ImageContents(ImageContents)` — one image's bytes as base64 plus its media type, for the Markdown preview. `InvalidRequest` for a path without an image extension (checked before the path is touched) and for a file over `fs_service::MAX_IMAGE_BYTES` (8 MiB), which is refused rather than cut. |
| | `WriteFile { workspace_id, path, text, expected_revision }` | `Ack`. `PreconditionFailed` when the on-disk content no longer matches the revision (an agent wrote the same path); the GUI re-reads with `ReadFile`. |
| | `CreatePath { workspace_id, path, directory }` | `Ack` after creating an empty entry and missing parents; refuses occupied paths and escapes. |
| | `RenamePath { workspace_id, from, to }` | `Ack` after an exclusive move inside the checkout. Root, identical paths, occupied destinations and moves into descendants are refused. Confirmed moves retarget editors and broadcast `FileChanged` for both paths. |
| | `DeletePath { workspace_id, path }` | `Ack` after deletion; directories are recursive, final symlink entries are removed without following them. Root is refused. There is no trash or undo. |
| | `SearchFiles { workspace_id, query, kind, limit }` | `Response::SearchResults(SearchResults)` — fuzzy name match, fixed-string `git grep -F` content search, or `Definition`: a `git grep -w -F` for the bare word kept only where the line declares it. The answer echoes `query`, which is what tells a client whose lookup it is. |
| | `WatchFiles { workspace_id, directories }` | Replaces this **connection's** directory watches and answers `Ack`; an empty list releases them. Directories are workspace-relative and non-recursive, capped before they are watched (128 per connection, 4096 bytes per path) and must canonicalize inside the checkout — a path that escapes is refused, one that no longer resolves is skipped, since the listing the client watched from is always a moment older than the checkout. Native events are coalesced (~150 ms, at most 32 paths per batch) and reported as `FileChanged`; a lost event becomes one empty-path invalidation. The `Ack` is not itself news: the set is replaced in place with no gap, so a client reconciles after an *arm* (first watch, new checkout, reconnect, retry) and not after every reply. The subscription belongs to the connection and dies with it. |
| Sessions | `CreateShellSession { workspace_id, parent, role }` | `SessionCreated` (state `Starting`, then `SessionUpdated` → `Running`). |
| | `CreateAgentSession { workspace_id, provider_id, profile_id, parent, role, resume, initial_prompt, read_only }` | Same; `ProviderNotInstalled` if detection failed. `profile_id` applies a launch profile (§13.4): `NotFound` if it is gone, `InvalidRequest` if it belongs to another provider. `resume` is a provider session id to re-enter instead of starting fresh (§13.5): `InvalidRequest` when that provider declares no way to resume. `initial_prompt` is a one-shot task string when the provider supports it (PR compose, PR review). `read_only` launches the provider in its own read-only mode (§16.9), which is what an automatic pull-request review runs in: `InvalidRequest` when the provider declares none, and unlike the prompt it is re-applied on a restart. A restart of the session re-enters the same conversation. |
| | `CreateEditorSession { workspace_id, path, line, read_only, autosave }` | Spawns `forge-editor` under the daemon's PTY (feature 19) and answers `SessionCreated` at spawn; the handshake and buffer open follow over the editor's own control socket, with `SessionUpdated` carrying `Session.editor` state. A save travels back the same way and the daemon writes it through `fs-service`, so `read_only` is a real choice and is enforced daemon-side. `NotFound`/`InvalidRequest` for a missing, escaping, directory, binary or too-large target (same codes `ReadFile` uses), `SpawnError` when `forge-editor` is not installed. Never restarts from a remembered path. |
| | `GetEditorConflict { session_id }` | The two sides of a refused editor save → `EditorConflict { path, disk, mine }`, a synchronous read like `GetWorkspaceDiff`. `NotFound` when that session has no standing conflict, which is the ordinary case. The documents travel only here — `Session.editor.conflict` is the flag that says it is worth asking. |
| | `ReloadEditorBuffer { session_id }` | Take disk: re-read the file and replace the editor's buffer, discarding the draft → `Ack`. Read fresh rather than reusing the conflict's copy, because a third writer may have landed since. |
| | `OverwriteEditorBuffer { session_id }` | Keep mine: ask the editor to send its draft again → `Ack`. The daemon learned the disk's revision when it refused, so that write is the one that lands. |
| | `SetEditorAutosave { session_id, autosave }` | Turn saving-on-a-pause on or off for a live editor session → `Ack`. The caller's own preference; the daemon carries it across and holds no opinion about it. |
| | `RevealInEditorSession { session_id, line, column }` | Moves the caret in a live editor session → `Ack`. What a second jump into an already-open file does, instead of opening a rival session. `NotFound` when no editor holds that id, `PreconditionFailed` when its command queue is saturated — the caller retries rather than the request id being dropped. |
| | `CreateChildSession { parent_session_id, kind, provider_id, profile_id, role, workspace_policy, initial_prompt }` | Child under a parent; workspace chosen by `ChildWorkspacePolicy`: `SameWorkspace`, `ExistingWorkspace`, or `NewManagedWorktree { branch_hint, base }`. The latter creates the workspace first and generates a branch when the hint is absent or blank. `initial_prompt` seeds an agent child. |
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
| | `SaveAgentProfile { profile }` | Create or replace a launch profile; the client supplies its id. `Conflict` for a duplicate name, `InvalidRequest` for a config directory the provider does not support, `ProviderNotInstalled` when its own executable fails the version probe. Profiles declare a config directory and arguments, not arbitrary environment variables. `AgentProfilesChanged`. |
| | `RemoveAgentProfile { profile_id }` | Deletes it; sessions it already started keep running. `AgentProfilesChanged`. |
| Shared files | `DetectShareCandidates { project_id }` | `Response::ShareCandidates` — the project's ignored paths, classified, with the strategy Forge would propose. One `git status --ignored=matching` plus a bounded walk: local and **synchronous** like `ListBranches`, and `truncated` says the list is a floor rather than the whole tree. |
| | `SetProjectShares { project_id, rules }` | Replaces the project's whole rule set (§14.2). The set, not a row: adding, reordering, enabling and re-pointing a strategy are one edit of one list. `InvalidRequest` for a path that is absolute, contains `..`, or names the git directory. `ProjectSharesChanged`. |
| | `RemoveShareRule { project_id, rule_id, cleanup }` | Drops one rule and says what happens to the files it already wrote: `Leave`, `RemoveInjected` or `Materialize`. Answers `Response::SharePlan` with what the cleanup did per workspace. Its own request because it is the only edit that can delete a file. `ProjectSharesChanged`. |
| | `PreviewShares { workspace_id }` | `Response::SharePlan` — what applying would do. Writes nothing. |
| | `GetShareStatus { workspace_id }` | `Response::ShareStatus` — per rule: `Applied`, `Missing`, `Severed` (a link a tool replaced with a real file), `Diverged`, `Skipped` or `Failed`. One `lstat` per rule, no subprocess. |
| | `ApplyShares { workspace_id, only }` | Acks when the work **starts**; the outcome arrives as `SharesApplied`. Coalesced per workspace, like `FetchRemote` — a `Run` rule is a subprocess and the GUI channel that carries this also carries every keystroke. `only: Some(ids)` is the user asking for those rules by name, which is the only case where something already on disk may be backed up and replaced. |
| | `AdoptIntoShareStore { project_id, path }` | Moves a real file into `<git common dir>/forge/shared` and leaves a symlink, backing the original up first. Explicit because it is the only operation that changes the primary checkout. |
| | `MaterializeFromShareStore { project_id, path, workspace_id }` | The inverse: writes the store file back as a real file. `workspace_id: None` does it for every workspace of the project. |
| Worktree ignores | `ListWorktreeIgnores { project_id }` | `Response::WorktreeIgnores(Vec<WorktreeIgnore>)` — the project's ignore rules, served from the daemon's cache. Local and synchronous; no scan, no broadcast. |
| | `SetWorktreeIgnores { project_id, rules }` | Replaces the project's whole ignore set (§14.4). Paths are resolved and canonicalized by the daemon; `InvalidRequest` for more than 200 rules or for a path that would hide the project's own checkout, and the daemon fills in `created_at`. The project is rescanned before the `Ack`, so rows the new rules cover are already gone; `ProjectWorktreeIgnoresChanged`. |
| | `ListProviderUsage` | `Response::ProviderUsage(Vec<ProviderUsage>)` — each *account's* remaining allowance, read from its own endpoint or CLI (§16.2): the default login of every provider that declares a source, plus one reading per launch profile that moved the config directory, each stamped with its `profile_id`. Re-read rather than served from the cache, and also broadcast as `ProviderUsageChanged` so every client sees the same reading. A provider that declares no source is simply absent. |
| | `GetUsageAnalytics { window_days }` | `Response::UsageAnalytics(Box<UsageAnalytics>)` — what the agents on this machine actually spent, counted off their own transcripts (§16.2). Local and **synchronous**, like `GetWorkspaceDiff`, and a read in the same sense: nothing is broadcast and nothing is stored. The daemon caches the scan for 60 s per window, because it is line-by-line IO over every recent transcript. `window_days: None` takes the reader's default (30) and an over-large value is clamped, never refused. |

### Change workflows and headless editor

| Request | Answer / side effect |
|---------|----------------------|
| `GetChangeContext { workspace_id }` | Synchronous `Response::ChangeContext`: status and patch context for drafting. |
| `GetSessionChanges { session_id }` | Synchronous `Response::SessionChanges`: summary since the recorded baseline, without patches. |
| `GetWorkspaceReview { workspace_id, context_lines }` | Synchronous `Response::WorkspaceReview`: one checkout diff against the common baseline of its sessions. |
| `GetExternalTranscript { session_id, provider, profile_id, max_turns, max_bytes }` | Synchronous `Response::ExternalTranscript`; resolves a discovered run by identity rather than accepting an arbitrary path. |
| `DeleteExternalSession { session_id, provider, profile_id }` | `Ack` after deleting that run's transcript/artifacts; refuses stores that cannot safely remove one run. |
| `DraftWithJuva { workspace_id, kind }` | `Ack` on start, then `JuvaDraftReady`; coalesced per workspace. Remote drafting falls back to deterministic local text. |
| `GetRebaseState { workspace_id }` | Synchronous `Response::RebaseState` read from Git's current sequencer/index state. |
| `ContinueRebase { workspace_id }` | Synchronous `Response::RebaseState`, including a replay that stopped at its next conflict. |
| `AbortRebase { workspace_id }` | Local mutation, `Ack`; refreshes workspace status. Discards resolutions made during the stopped operation. |
| `MarkConflictResolved { workspace_id, paths }` | Stages literal checkout-relative paths, then answers `Response::RebaseState`. |
| `CreateCommit { workspace_id, message }` | Stages all changes and creates a local commit, then `Ack` and workspace status refresh. Does not push. |
| `CreatePullRequest { workspace_id, title, body, base }` | `Ack` on start, then `PullRequestOpened`; explicitly pushes before opening through `gh`, coalesced per workspace. |
| `SendEditorInput { session_id, events }` | `Ack` after queueing bounded structured input for the DOM editor; `PreconditionFailed` on a saturated editor queue. |
| `SetEditorView { session_id, first_line, line_count }` | `Ack` after queueing a bounded line-window request; rendered content arrives in `EditorFrame`. |
| `EditorFind { session_id, command }` | `Ack` after queueing a find-panel gesture (`Set`, `Next`, `Previous`, `Close`); the result arrives as `EditorState.find`. A `Set` pattern over 1 024 bytes is `InvalidRequest`. |

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
| `FactoryReset` | Discard the metadata replica and bootstrap again. |
| `ClipboardStore { terminal_id, text }` | A terminal's OSC 52 clipboard write; clipboard reads are not exposed to the child. |
| `EditorFrame { session_id, frame }` | A bounded line window for the passive DOM editor surface. |
| `ProviderUsageChanged { usage }` | Replaces all account allowance readings; never merge with the old set. |
| `JuvaDraftReady { workspace_id, draft, fell_back }` | Completion of `DraftWithJuva`, with editable text and whether a configured remote endpoint fell back. |
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
| `FileChanged { workspace_id, path }` | Native watch notifications go to the watching connection; successful Forge renames also broadcast invalidations for both paths. An event is not proof of watch coverage. Carries no content; clients reconcile relevant reads without discarding dirty buffers. `path` is workspace-relative; an **empty** path asks the client to resync its visible surface. An undeliverable native notification becomes an owed resync, retried rather than disconnecting. |
| `AgentDetectionChanged { results }` | After startup detection or a refresh. |
| `AgentProfilesChanged { profiles }` | The whole set after any profile write (§13.4) — like usage and detection, a merge would strand a deleted profile. |
| `ProjectSharesChanged { project_id, rules }` | One project's whole rule set after any write (§14.2), for the same reason as the profiles event. |
| `ProjectWorktreeIgnoresChanged { project_id, rules }` | One project's whole ignore set after `SetWorktreeIgnores` (§14.4) — and when a rescan collects a tombstone, so the GUI's list matches the daemon's. |
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
- `events()` returns a bounded receiver (64 events). The reader uses nonblocking
  delivery; overflow disconnects so the GUI reconnects for a fresh snapshot.
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
