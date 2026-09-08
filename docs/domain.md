# Domain model

The `domain` crate is the contract every other crate builds on. It depends only
on `serde`, `uuid`, `time`, `thiserror` and `compact_str`, and it contains **no
behavior beyond validation** — no I/O, no Git, no PTY. Everything in it is
`Serialize + Deserialize` so the same values travel over IPC, live in the
daemon's memory and map to SQLite columns.

Source: `crates/domain/src/`. Plan references: §7, §8, §11.4.

## Glossary

| Term | Meaning |
|------|---------|
| **Project group** | A named organizational container shown as a "Workspace" in the sidebar. It groups projects but has no path and never runs sessions. |
| **Project** | A directory the user added. May or may not be a Git repo. |
| **Workspace** | A directory where sessions actually run: the main checkout *or* a Git worktree. The main checkout is never a special case. |
| **Session** | A persistent unit of work — a shell or an agent CLI — and a node of the session graph. Survives daemon restarts as metadata. |
| **Terminal** | The live PTY + VT engine behind a session. Runtime-only; a `TerminalId` is regenerated on every spawn/restart and never persisted. |
| **Agent provider** | A coding-agent CLI (`claude`, `codex`, `opencode`, `cursor`). Described declaratively by an `AgentDescriptor`. |
| **Session graph** | Logical parent/child relationships between sessions (a session can spawn sessions). Independent of the OS process tree. |
| **Context envelope** | An explicit, auditable hand-off of context from one session to another. Schema only in the MVP. |
| **Managed worktree** | A worktree Forge created itself (`managed_by_app = true`). Only these are ever removed from disk. |

## Identifiers (`ids.rs`)

- `ProjectGroupId`, `ProjectId`, `WorkspaceId`, `SessionId`, `TerminalId`, `ClientId`,
  `ContextId` are newtypes over **UUID v7**. v7 is time-ordered, so sorting by
  id gives creation order for free (sidebar ordering, session lists).
- `AgentProfileId` is a UUID v7 too: a launch profile (§13.4) is a saved row
  the user can rename, so its identity cannot be its name.
- `AgentProviderId` is a plain string (`"claude"`, `"cursor"`, …) so
  descriptors and overrides can key on a stable, human-readable value.
- `Timestamp` wraps `time::OffsetDateTime` in UTC and serializes as RFC-3339;
  the same string is what SQLite stores.

## Entities

```
ProjectGroup 1 ──< Project 1 ──< Workspace 1 ──< Session
                       │                              │
                       │                              └── terminal_id (runtime only)
                       └── project_group_id is optional (General when absent)
```

### `ProjectGroup`

A `ProjectGroup` is organizational only. The UI calls it a Workspace because
that is the user-facing collection of related projects; the existing
`Workspace` domain entity keeps its execution-specific meaning as a checkout or
Git worktree. Deleting a group leaves its projects intact and moves them to
General through `ON DELETE SET NULL`.

### `Project` (§7.1)

| Field | Notes |
|-------|-------|
| `project_group_id` | Optional organizational parent; `None` appears under General. |
| `root_path` | Canonicalized absolute path; unique across projects. |
| `git_root` | `None` for non-Git directories — the project is still valid, worktree actions are simply disabled. |
| `name` | Defaults to the basename; user-editable via `RenameProject`. |
| `icon` | The glyph the user picked to stand for the project, set via `SetProjectIcon`. `None` — where every project starts — means the UI draws the project's initials. An opaque string, not an enum of known icons: the point is that the mark is the user's to choose. `domain::is_valid_icon` is the shared rule for what can be stored (one drawable mark, no whitespace, at most `MAX_ICON_CHARS`), and both the GUI and the daemon apply it. |

### `Workspace` (§7.2)

| Field | Notes |
|-------|-------|
| `kind` | `Main` (main checkout or plain directory) or `GitWorktree`. `#[non_exhaustive]` — a remote kind is anticipated. |
| `path` | Unique across workspaces. |
| `branch` | `None` when detached/unborn or not Git. |
| `display_name` | Optional human label, independent of the branch. When set, the sidebar leads with it and keeps the branch as a secondary line. Cleared by `RenameWorkspace`. |
| `managed_by_app` | `true` only for worktrees Forge created. Gate for any on-disk removal (§14.4). |
| `status` | `WorkspaceStatus { dirty, ahead, behind, measured_at }`. **Runtime-only**, like `Session::terminal_id`: never a column, and `Default` (unmeasured) on load. `measured_at: None` means "nobody has looked", which the sidebar draws as nothing rather than as "clean". |

### `BranchRef` (§14.3)

What the branch picker lists. Reported by the daemon on every `ListBranches`,
never cached: a stale row is then a stale answer to an old question rather than
a wrong model.

| Field | Notes |
|-------|-------|
| `name` | Short name, with neither `refs/heads/` nor the remote prefix. `feature/auth` for both a local branch and `origin/feature/auth`. |
| `scope` | `Local`, or `Remote { remote }`. `#[non_exhaustive]`. |
| `upstream` | What a local branch tracks (`origin/main`). Always `None` for a remote ref — one *is* an upstream. |
| `committed_at` / `subject` | Tip commit date (the list's sort key, newest first) and subject line. |
| `checked_out_in` | The workspace already holding this branch, if any. Git refuses a second checkout, so the picker offers such a row disabled, naming the worktree, instead of letting the user hit a `Conflict`. |

`start_point()` gives the ref to branch from: the bare name for a local branch,
`origin/<name>` for a remote one — which is what makes git configure the
upstream by itself.

### `Session` (§7.3, ADR-010)

| Field | Notes |
|-------|-------|
| `kind` | `Shell` or `Agent`. |
| `role` | `Generic` by default; `Orchestrator`, `Planner`, `Researcher`, `Executor`, `Reviewer`, `Tester`, `Custom(String)` are tags for the future orchestrator and carry no behavior today. |
| `parent_session_id` / `root_session_id` | The graph. Roots have `parent = None` and `root == id`. |
| `terminal_id` | `Some` only while a PTY exists. `None` in `Failed`/`Orphaned` and while a restart is pending. |
| `agent_provider_id` | `Some` for `Agent` sessions. |
| `agent_profile_id` | `Some` when the session was launched from a profile (§13.4). Kept even after that profile is deleted: the row is history, and the GUI falls back to the provider's name. |
| `title` | `SessionTitle { user, terminal }`; see the title rule below. |
| `state` | The state machine below. |
| `last_activity_at` | Last PTY output or input. **Runtime-only**, like `terminal_id`: never a column, and reconstructed as `ended_at` (else `created_at`) on load. `idle_for(now)` and `age(now)` read it; see the idle policy below. |

#### State machine

```
            ┌──────────────────────────────────────────┐
            │                                          │
  Starting ──► Running ──► Exited { code, signal }      │
     │                 └─► Orphaned                     │  RestartSession
     └──► Failed { reason }                             │
                                                        │
  Exited | Failed | Orphaned ───────────────────────────┘
```

`SessionState::can_transition_to` is the single validator; the daemon calls it
before every mutation. Derived predicates:

- `is_active()` — `Starting | Running`. `CloseSession` is **rejected** in these
  states; the client must `KillSession` first.
- `is_terminal()` — `Exited | Failed | Orphaned`. Only these accept
  `RestartSession`.

There is no separate "exited" event: exit code/signal ride on
`SessionState::Exited`, and every change is delivered as one
`DaemonEvent::SessionUpdated`.

`Orphaned` is set by the daemon at startup for any session that was
`Starting`/`Running` when the previous daemon died — a PTY never survives the
daemon (§15.3). It only appears when `sessions.persist_history` is on; the
default drops those rows instead of keeping them.

#### Idle and age

Two clocks describe a live session, and they answer different questions:

- `idle_for(now)` — since `last_activity_at`: **nobody is using it**. Any
  terminal output or input resets it.
- `age(now)` — since `created_at`: **open for too long**. It keeps growing while
  the session is busy.

The daemon bumps `last_activity_at` at most once a second per terminal and never
writes it to SQLite; `crates/daemon/src/idle.rs` turns the two clocks plus
`[sessions]` config into a warn/stop verdict. Inactivity can warn and (opt-in)
stop; age only ever warns.

#### Title rule

`SessionTitle::resolve(fallback)` returns, in order: the user title set via
`RenameSession`, else the latest OSC 0/2 title reported by the terminal, else
the caller's fallback (`"Shell"` or the provider display name).

#### Graph rules (enforced in the daemon, `core.rs`)

- A child must belong to the **same project** as its parent (another workspace
  of that project is fine).
- No cycles; depth is capped at `MAX_GRAPH_DEPTH = 8` (root is depth 1).
- Closing a session **re-parents its children to the grandparent** and
  recomputes their `root_session_id`; the graph is never torn down by closing a
  middle node.
- The graph is *logical*. Process-level cleanup is handled separately by
  signaling the whole process group (see [terminal.md](./terminal.md)).

### `ChildWorkspacePolicy` (§8.2)

Where a child created via `CreateChildSession` runs:

- `SameWorkspace` — the parent's workspace.
- `NewManagedWorktree { branch_hint, base }` — Forge creates a worktree first.
- `ExistingWorkspace(WorkspaceId)` — any workspace of the same project.

### `ContextEnvelope` (§8.3)

`source_session_id`, optional `target_session_id`, `summary`, `instructions`,
`artifacts: Vec<ContextArtifactRef>` (kinds: `Text`, `Plan`, `Review`,
`FileReference`, `DiffReference`, `CommitReference`, `TerminalExcerpt`,
`StructuredJson`) and an optional `GitContextRef { repo_path, branch, commit }`.

MVP status: the type, the table and `CreateContextEnvelope` exist so the
migration is in place before orchestration; only `summary`/`instructions` are
populated and no UI consumes envelopes.

### `ExternalAgentSession` (`external.rs`)

A read-only agent run **the daemon never launched**, recovered from the CLI's
own transcript on disk. It is not a node of the session graph: no PTY, no
lifecycle, no row in the database. The daemon recomputes the set from disk on
every `GetSnapshot` (see [agents.md](./agents.md#discovered-history)), so it
never persists and never goes stale in the replica.

| Field | Meaning |
|-------|---------|
| `session_id` | the provider's own id, stable across snapshots so the GUI can key cards on it |
| `project_id` / `workspace_id` | where the run happened. `workspace_id` is `Some` when the recorded directory is one Forge tracks — that is what lets the history panel scope to a single worktree |
| `provider` | slug (`claude`, `opencode`); drives the glyph and label |
| `title` | the provider's generated title, else the first user prompt, else the id |
| `branch` | the Git branch recorded in the transcript, when it records one |
| `preview` / `model` | the agent's last readable message, folded to one paragraph, and the model that wrote it |
| `message_count` / `subagent_count` | conversation turns (tool traffic excluded) and subagent runs spawned |
| `transcript_path` | the file, for the GUI to open when the run cannot be resumed |
| `started_at` / `last_activity` | first and last recorded activity, falling back to the file's mtime |

### `PullRequest` (`pull_request.rs`)

An open pull request read from a forge host. Like `ExternalAgentSession` it is
**cached remote data, never authoritative**: no table, no migration, no row that
can outlive the truth. The daemon rebuilds the whole set on `RefreshPullRequests`
and serves the last one — unchanged — from `GetSnapshot`, so a snapshot never
waits on the network.

| Field | Meaning |
|-------|---------|
| `project_id` | the Forge project whose remote produced it, or `None` for a personal search hit in a repository Forge does not track |
| `repository` / `host` | `owner/name` and the forge hostname; together with `number` they are the row's identity |
| `title` / `body` / `body_truncated` | the description is capped at `MAX_PR_BODY` (4000 bytes, cut on a UTF-8 boundary) — the panel shows it as context, and the browser has the rest |
| `url` | where the ↗ action goes |
| `author` / `base_ref` / `head_ref` / `is_draft` | the header line of a card |
| `review_decision` | `Approved` / `ChangesRequested` / `ReviewRequired`, `None` when the host reports none |
| `labels` / `assignees` / `review_requests` | as the host lists them; logins are raw, never compared in the GUI |
| `additions` / `deletions` / `changed_files` / `comment_count` | the size hints on a card |
| `relations` | `PullRequestRelations { assigned, review_requested, authored }`, derived **in the daemon** from the same response's `viewer.login`. The GUI filters on `is_mine()` and never compares account names itself — the same rule that keeps provider knowledge inside `crates/agents`. |

`PullRequestState` is what actually travels: the pull requests plus `viewers`
(one login per queried host, so the panel can say *which* account answered),
`sources` (why a project contributed nothing — `NoGitRepository`, `NoRemote`,
`UnsupportedHost`, `InvalidRemote`, `Failed`), `failures` (per-repository errors
that did not sink the rest) and `error` (set only when every host failed).
A host is eligible when it is `github.com` or is listed in
`[github] enterprise_hosts`: remote-URL syntax alone cannot tell GitHub
Enterprise from GitLab, so it is never guessed.

### `ReviewRecipe` (`pr_review.rs`)

What to ask an agent for when reviewing someone else's pull request (§16.9).
The sibling of `PrTask` (`pr_task.rs`), and deliberately not the same thing: a
task is about your own uncommitted work on the way to *opening* a pull request,
a recipe is about one that already exists. The difference is what the agent may
do — a task writes, a review does not.

Built-in recipes live in code like the agent descriptors do, and travel with
the launch: no table, no migration, no daemon state. `compose_review_prompt`
joins the recipe, the `gh` commands that reach the pull request, a small
identity block, and whatever the user added.

| Field | Meaning |
|-------|---------|
| `id` | stable key, for remembering the last choice — `standard`, `deep`, `security`, `custom` |
| `name` / `detail` | what the menu entry says, and one line on what picking it does |
| `body` | the wording sent before the context; empty for `custom`, which is the user's own text and nothing else |

Two independent guards keep a review a read: the launch applies the provider's
own `ReviewStyle` flags, and every recipe's prompt repeats the rule in prose —
no editing, no committing, no `gh pr review`. A model that ignores one still
meets the other.

### `WorkspaceDiff` (`diff.rs`)

One checkout's uncommitted changes, a unified patch per file. Runtime-only for
the same reason as `PullRequest` and `Workspace::status`: it describes a working
tree at one instant, and a stored copy would describe one that no longer exists.
It is computed on demand by `GetWorkspaceDiff` and never broadcast — a diff is a
view one client asked for, not shared state.

| Field | Meaning |
|-------|---------|
| `workspace_id` / `branch` | which checkout, and the branch it is on when not detached |
| `files` | changed paths in git's own order |
| `truncated` | files, or one file's patch, were left out of the answer |

### `FileTree` / `FileContents` / `SearchResults` (`file.rs`)

Workspace filesystem views for the in-app editor (ADR-012). Runtime-only like
`WorkspaceDiff`: computed on demand by `ListFiles` / `ReadFile` / `SearchFiles`,
never stored, never broadcast. `FileContents.revision` is the optimistic-
concurrency token `WriteFile` must present back. `SearchResults.query` echoes
what was asked: the answer arrives as an event with no request id, so it is the
only thing that tells a second lookup from the first one's answer.

`SearchKind::Definition` is not a content search with a crafted pattern. The
service greps for the bare word (`git grep -w -F`, so a symbol from a click can
never become a regex) and keeps the lines whose *shape* declares it — a
declaring keyword before the name, a binding keyword, or a bare signature that
opens a block. It is a heuristic, and the GUI presents several answers rather
than choosing one.

Each `DiffFile` carries `path`, `status` (`Added` / `Modified` / `Deleted`,
decided by the *worktree* column of `git status` so a staged-then-deleted path
reads as gone), `additions` / `deletions`, `binary`, and the `patch` itself.
A patch that breaks a budget is dropped **whole** and flagged `truncated`: half a
patch is not a patch, and a viewer that rendered one would show a lie.

## Agent runtime types (`agent.rs`)

| Type | Role |
|------|------|
| `AgentDescriptor` | Declarative provider definition: `binary_candidates` (in preference order), `default_args`, `version_probe`, `capabilities`, `profile_fields`, `resume`. Owned `String`s so it can travel over IPC and support custom agents later. |
| `AgentProfile` | A named way to start a provider (§13.4): `provider_id`, `name`, optional `executable`, `args`, and an `env` **overlay** (unlike `SpawnSpec.env`, which is complete). |
| `ProfileField` / `ProfileFieldEffect` | What the profile editor offers for a provider, as data: `Env { name, is_directory }` or `Flag { flag }`. `is_directory` is also what makes the daemon create the directory before launching. |
| `VersionProbe` | `args` (`--version`), optional `expect_substring` to reject look-alike binaries, `timeout_ms`. |
| `AgentCapabilities` | `interactive_tui`, `supports_initial_prompt` — informational. `supports_resume` restates whether the descriptor carries a `ResumeStyle`. |
| `ResumeStyle` | How a provider re-enters an earlier session of its own (§13.5): `Flag { flag }` (`claude --resume <id>`) or `Subcommand { command }` (`codex resume <id>`). `None` on the descriptor means its history is read-only. |
| `DetectionResult` / `DetectionStatus` | `Installed { executable, version }`, `NotFound`, `Rejected { candidate, reason }`, `ProbeTimeout`. |
| `ResolvedEnvironment` | The login-shell environment resolved once by the daemon (`EnvSource::LoginShell` or `ProcessFallback`). |
| `LaunchAgentRequest` | `provider_id`, `cwd`, `extra_args`, `executable_override`, `resume_session_id`. |
| `SpawnSpec` | Fully resolved `program`, `args`, `cwd`, and a **complete** `env` (not an overlay — the PTY launch clears the inherited environment first). |
| `PtySize` | `cols`, `rows` plus pixel dimensions; default 80×24. |

See [agents.md](./agents.md) for how these are used.

## Usage types (`agent.rs`, `usage.rs`)

Two readings, deliberately kept apart. Merging them would produce a number that
answers neither question.

| Type | Role |
|------|------|
| `UsageSource` / `UsageProbe` | Where a provider's *allowance* reading comes from: a CLI printing one documented JSON document, or its existing local OAuth credentials (`ClaudeOauth`, `CodexOAuth`). |
| `ProviderUsage` / `UsageWindow` | The provider's own word on how much of a rolling window is gone: a whole `used_percent` (no float in a wire type), a human `window` label, an optional `resets_at`, and `collected_at` so a stale reading can be shown as stale. Empty `windows` means "reported nothing", rendered as no meter — never as 0%. |
| `UsageAnalytics` | *Our* count, from the transcripts the CLIs write to disk: `providers`, activity-only `daily` buckets, the `window_days` scanned, and `scanned`/`skipped` so a bounded scan never looks exhaustive. |
| `ProviderAnalytics` | One provider's totals: `tokens`, `sessions`, `turns`, `cost_micros`, `unpriced_turns` (a non-zero count means the cost is a floor), `top_model`, `worked_secs`, and the first/last activity seen. |
| `TokenTotals` | `input`, `output`, `cache_write`, `cache_read`, `reasoning`. `reasoning` is a subset of `output` and is *not* in `total()`; the other four are disjoint. |
| `DailyUsage` | One UTC day's token total, keyed `YYYY-MM-DD`. A day with no activity is absent rather than zero — the series is a list of facts and the view fills the gaps. |

Money is counted in micro-dollars (`MICROS_PER_USD`), never in `f64`: the
protocol stays `Eq`. Which models have a published price is a provider-specific
fact and lives in `agents`, like every other one.

## Terminal wire types (`terminal.rs`, §11.4)

These are the data the daemon's engine produces and the GUI renders. They live
in `domain` rather than `terminal-core` precisely so that `ui`/`client` never
depend on the emulator (§17).

| Type | Notes |
|------|-------|
| `Cell` | `text` (full grapheme cluster, `CompactString`), `fg`, `bg`, `flags`. Wide chars: the first cell has `WIDE_CHAR`, the continuation has `WIDE_SPACER` and empty text. |
| `CellFlags` | `u16` bitset: `BOLD`, `ITALIC`, `UNDERLINE`, `INVERSE`, `DIM`, `STRIKEOUT`, `HIDDEN`, `WIDE_CHAR`, `WIDE_SPACER`. Serialized as the raw integer. |
| `Color` | `Default`, `Indexed(u8)` (named 0–15 fold into this) or RGB. |
| `Row` | `cells` + `wrapped` (soft wrap into the next row). |
| `Cursor` | `line`, `col`, `shape` (`CursorShape`), `visible`. |
| `TermModes` | `alt_screen`, `bracketed_paste`, `app_cursor_keys`, `app_keypad`, `mouse_mode` (`MouseMode`), `mouse_sgr`, `focus_events` — everything input mapping and rendering need. |
| `TerminalSnapshot` | `seq`, `size`, `visible` rows, `scrollback_tail` (last `DEFAULT_SCROLLBACK_TAIL = 200` lines), `scrollback_len`, `cursor`, `modes`, `title`. Sent on attach and resync. |
| `TerminalDelta` | `seq`, `rows: Vec<(u16, Row)>` (only damaged rows), `scrolled_lines`, `cursor`, `modes`. |
| `ScrollbackRows` | `from_line` + `rows`, the answer to `FetchScrollback`. |

The sequence/resync protocol built on `seq` is described in
[terminal.md](./terminal.md).

## Conventions

- Enums that may grow (`WorkspaceKind`, `SessionKind`, `SessionState`,
  `SessionRole`, `DetectionStatus`, `Color`, `Damage`, …) are
  `#[non_exhaustive]`; cross-crate `match`es need a wildcard arm.
- IDs implement `Display`/`FromStr` for the UUID string form, which is also the
  SQLite representation.
- Nothing in `domain` branches on a provider id; provider-specific facts live in
  `crates/agents` (principle P2).
