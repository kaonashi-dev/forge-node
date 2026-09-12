# Projects, workspaces and worktrees

A **worktree is an isolated workspace**: several agents can work on the same
repository concurrently, each on its own branch and directory, without
stepping on each other's files. The main checkout is just another
`Workspace` (`kind = Main`) — nothing in the code special-cases it (P4).

Crates: `git-service` (Git CLI wrapper), `daemon/src/core.rs` (the
`ProjectService`/`WorkspaceService` logic). Plan references: §14. ADR-008 (Git
CLI first).

## Git invocation rules (ADR-008, `git-service/src/command.rs`)

Every application Git call goes through `git_service::run_git`, which:

- runs `git -C <repo> …` as a **background subprocess**, never inside a user
  PTY;
- forces `LC_ALL=C` (parseable output) and `GIT_TERMINAL_PROMPT=0` (no
  credential prompts hanging the daemon);
- captures stdout/stderr/status; non-zero exit is still a `GitOutput` —
  callers decide whether it is an error (`GitOutput::ok`) or a signal (e.g.
  `rev-parse --show-toplevel` failing means "not a repo");
- enforces a **30 s timeout** (`GIT_TIMEOUT`), killing the child with
  `SIGKILL` and returning `GitError::Timeout`.

Test fixtures (`test-support::temp_repo`) may call `git` directly; application
code must not.

## Project discovery (§14.1, `repository.rs`)

`AddProject { path }`:

1. Canonicalize `path`; reject a duplicate `root_path`.
2. `discover_root` (`rev-parse --show-toplevel`) → `git_root`, or `None` for a
   plain directory (still a valid project, worktree actions disabled).
3. `current_branch` (`rev-parse --abbrev-ref HEAD`; `None` when detached).
4. Create the `Main` workspace at `root_path` with `managed_by_app = false`.
5. `list_worktrees` (`worktree list --porcelain`) → one `GitWorktree`
   workspace per pre-existing worktree, **`managed_by_app = false`** (Forge
   will never delete them from disk).

`RefreshWorkspaceStatus` runs `status --porcelain=v2 --branch` and updates the
workspace's `branch` **and** its `WorkspaceStatus` (head oid / dirty / ahead /
behind), throttled to one run every 2 s per workspace (ADR-008). The GUI calls
it when a session is selected — the moment its checkout becomes visible.

`RefreshProject` reconciles **on demand**: it re-runs `discover_root`, then
`rescan_project_worktrees` (the per-project half of the startup rescan), then
one `git status` per workspace of that project. So a worktree created in a
terminal shows up the moment the user asks Forge to look — which is exactly
when they ask, because that is the state in which Forge and the terminal
disagree.

Startup does the same sweep across every project: `Daemon::start` calls
`rescan_worktrees` before accepting clients (§15.3 steps 3 and 4). It runs
`git worktree list` per Git project, adds workspaces for worktrees created
outside the app and removes those that vanished, broadcasting
`WorkspaceCreated` / `WorkspaceRemoved`.

`Workspace.status` (`WorkspaceStatus { dirty, head, ahead, behind, measured_at }`) is
**runtime state, never a column** — the same rule as `Session::terminal_id`. A
workspace loaded from SQLite starts unmeasured, which the sidebar renders as
*nothing* rather than as "clean": a row that claims to be clean because nobody
looked is worse than a row that says nothing.

## Where managed worktrees live (§14.2)

```
<worktrees.root>/<project-id>/<slug>
```

`worktrees.root` defaults to `<data-dir>/worktrees`
(`~/Library/Application Support/Forge/worktrees` on macOS,
`$XDG_DATA_HOME/forge/worktrees` on Linux — `directories` lowercases the
application name there) and can be changed in
`config.toml`. The daemon creates the parent directory with mode `0700`.

### Slug rules (`worktree.rs::slugify`)

- `/` → `-`; any char outside `[A-Za-z0-9._-]` → `-`.
- Runs of `-` collapse to one.
- Capped at 64 ASCII characters.
- A result of `""`, `"."` or `".."` becomes `worktree` — the slug is a path
  component, so it must never be able to escape the project directory.
- `unique_slug` appends `-2`, `-3`, … on collision with the slugs of the
  project's **`managed_by_app`** workspaces (unmanaged ones live outside
  `worktrees.root` and cannot collide).

`feature/auth refactor` → `feature-auth-refactor`.

## Branches and remotes (§14.3)

`ListBranches { project_id }` → `Response::Branches { branches, remotes,
default_branch }`. It is a **local read**: `for-each-ref` over `refs/heads` and
`refs/remotes`, sorted `-committerdate`, plus `remote -v` and `origin/HEAD`. It
never opens a socket, so it is safe to call from the GUI thread that also
carries keystrokes.

Each `BranchRef` carries `checked_out_in: Option<WorkspaceId>`, filled in by the
daemon from its own workspace model. Git refuses to check the same branch out
twice, and a picker that offers the row anyway teaches the user to expect a
`Conflict` error; with this the row is offered **disabled, naming the worktree
that holds it**.

`refs/remotes/<remote>/HEAD` is dropped from the listing: it is a symbolic ref
standing for the remote's default branch, not a branch of its own.

### Fetch (`FetchRemote { project_id, remote }`)

Acks **immediately** and reports through `DaemonEvent::RemoteRefsUpdated
{ project_id, remote, updated, error }`. The ack means *started*, not *done*.

This is the only asynchronous request in the daemon, and the reason is on the
client side: the GUI drains one command channel on one thread, and that channel
also carries `RuntimeCommand::Input` — every keystroke. A synchronous fetch
would freeze typing for as long as the network took.

A fetch already in flight for the same project is **coalesced, not queued**
(`Inner::fetching`): the second request acks and rides the first one's result.

`remote: None` resolves to `origin`, falling back to the only remote configured
when it is named something else (`upstream`, a fork's `me`). A project with no
remote is refused rather than guessed at.

### Network commands deviate from ADR-008, deliberately

`git_service::run_git_network` exists alongside `run_git` because ADR-008's
single 30 s timeout was calibrated for local commands: for `rev-parse` 30 s
means something is broken, for the first `fetch` of a large monorepo it means
the repository is large. Network commands get their own budget
(`[git] fetch_timeout_secs`, default 120 s) and two hardenings `run_git` cannot
provide:

- **`GIT_TERMINAL_PROMPT=0` does not reach `ssh`.** It stops git's own prompt,
  but `ssh` reads the TTY directly, so an encrypted key with no agent loaded
  blocks until the timeout. `GIT_SSH_COMMAND` with `BatchMode=yes` (plus
  `StrictHostKeyChecking=accept-new`, `ConnectTimeout=10`) makes it fail
  immediately and say why.
- **Askpass helpers** are pointed at `/usr/bin/false` with
  `SSH_ASKPASS_REQUIRE=never`, so a daemon with no window cannot spawn a
  credential dialog.

The result is that a failed fetch reaches the GUI as `Permission denied
(publickey)`, not as `Timeout`.

Nothing in `git-service` writes to a remote. `push` is not implemented, and that
is a decision, not a gap: a daemon that can push is a daemon that can lose
someone's work from a background thread.

## Create (§14.3, `CreateWorktree { project_id, branch, base, name }`)

1. Project must be Git (`GitError` otherwise).
2. `validate_branch_name` via `git check-ref-format --branch`.
3. Slug from `name` if given, else from `branch`; path as above.
4. Then, depending on the branch:

| Branch state | Command |
|--------------|---------|
| does not exist | `git worktree add -b <branch> -- <path> <base>` (`base` defaults to `HEAD`) |
| exists, not checked out elsewhere | `git worktree add -- <path> <branch>` |
| exists, checked out in another worktree | `GitError::Conflict` carrying that worktree's path → `ErrorCode::Conflict` |

5. **Provision it** (see below).
6. Persist the `Workspace { kind: GitWorktree, managed_by_app: true }` and
   broadcast `WorkspaceCreated`.

The request is acked before that broadcast, so the GUI cannot open the new
checkout on the reply: `app_shell` keeps the branch it asked for in
`pending_worktree` and enters the workspace when it arrives with that branch on
it ([ui.md](./ui.md#branches-are-workspaces-143)). Matching by branch is sound
because the picker never offers one that is already checked out.

### Checking out a remote branch needs no `--track`

To start work on a branch that only exists on the remote, pass
`branch = "feature/x"` with `base = Some("origin/feature/x")`. That takes the
first row of the table (`worktree add -b feature/x -- <path> origin/feature/x`),
and because the start point is a remote-tracking branch git configures the
upstream **by itself** via `branch.autoSetupMerge`, which is on by default.
Verified against real git in
`git-service/tests/integration.rs::a_worktree_created_from_a_remote_ref_tracks_it`
and end to end in `daemon/tests/scenario_branches.rs`.

### Provisioning: shared files (§14.2)

A managed worktree lives under `worktrees.root`, away from the repository, so
it starts **without the untracked files a project needs to run**: `.env`, a
local settings file, a certificate, an installed `node_modules`. An agent
launched there fails on its first command, and nothing about that failure
points at the missing file.

What follows a checkout is a **rule**, not a list, because no single mechanism
is right for every file: a secret wants one source everyone sees, a dependency
directory wants isolation per branch, and a build cache wants neither.

```rust
ShareRule { path, strategy, enabled, position }
ShareStrategy = Copy | Clone | Link | Run { command, timeout_secs }
```

| Strategy | What it does | Where it fits |
|---|---|---|
| `Copy` | byte-for-byte copy from the source workspace, mode preserved, symlinks recreated as symlinks | small config a worktree may diverge on |
| `Clone` | copy-on-write clone (`clonefile` on APFS, `FICLONE` on Linux), falling back to a full copy and **reporting** it | `node_modules`, `.venv`, `target` — free at creation and isolated afterwards |
| `Link` | symlink into `<git common dir>/forge/shared` | one file every workspace sees, nothing duplicated at rest |
| `Run` | `sh -c` in the workspace when the path is missing | dependencies, where two branches can disagree on a lockfile |

**Where the files come from.** The source is the project's `Main` workspace —
the primary checkout, whatever branch it has out. `Link` rules are the
exception: their source is the store in the repository's **common** git dir
(`git rev-parse --git-common-dir`), which every workspace of the project
resolves to the same path, so no checkout is load-bearing for the others and
the store moves and dies with the repository. Seeding it moves the real file
and leaves a link, with a backup — the one operation that touches the primary
checkout, and never a side effect of an apply.

**Rules are the project's own** (`worktree_shares`, migration 8), edited in
Settings → *Shared files*. A project with none falls back to the global
`[worktrees] copy` / `setup_script`, synthesised into rules and run through the
same engine. A project with rules ignores the global list entirely: merging the
two would make it impossible to remove an inherited entry.

**When it runs.** On create, on adopt (a worktree made in a terminal needs the
files as much as one Forge created), and on demand from `ApplyShares`. Not on a
watcher: writing into a directory an agent is using has no correct resolution.

**It never runs on the request thread.** `CreateWorktree` now persists the
workspace, broadcasts `WorkspaceCreated` and *starts* provisioning; the outcome
arrives as `SharesApplied`. A `Run` rule is a subprocess of up to an hour, and
the GUI drains one command channel on one thread — the same channel that
carries every keystroke. The run is coalesced per workspace
(`Inner::provisioning`, released by a `Drop` guard) and never holds the core
lock across filesystem or subprocess work.

**Nothing is overwritten by a run nobody asked for.** A background apply skips
a path that already exists; only `ApplyShares { only: Some(ids) }` — the user
naming those rules — may back up what is there and replace it. Removing a rule
asks once what to do with what it wrote (`Leave`, `RemoveInjected`,
`Materialize`), `RemoveInjected` deletes only what is still recognizably
Forge's, and the source workspace's own file is never taken away.

**Safety.** Relative paths only, no `..`, nothing inside `.git`, symlinks in
the source recreated rather than followed, and copies budgeted **before** the
bytes are written (`MAX_COPY_BYTES`, `MAX_COPY_ENTRIES`) so a fallback copy of
`node_modules` cannot fill the disk. What Forge injects is added to the managed
block of `$GIT_COMMON_DIR/info/exclude`, so a provisioned worktree does not
read as dirty.

**Never fatal.** By the time this runs the worktree is on disk and checked out;
refusing to hand it over because a script exited 1 would leave the user worse
off than handing over a checkout that needs a second look. A rule that cannot
be applied becomes a `Skip` with a note, and problems become `DaemonNotice`s.

`ChildWorkspacePolicy::NewManagedWorktree` (§8.2) exists in the protocol but is
**not implemented**: `CreateChildSession` rejects it with `InvalidRequest`
("NewManagedWorktree child policy not wired in MVP UI path"). Only
`SameWorkspace` and `ExistingWorkspace` work today; creating the worktree first
with `CreateWorktree` and then the child in it is the available path.

## Remove (§14.4, `RemoveWorktree { workspace_id, force }`)

Safety first: **a branch is never deleted**, by any path.

```
not a GitWorktree ──► InvalidRequest
force == false:
    running sessions in the workspace
 or dirty tree (`status --porcelain`)
 or MERGE_HEAD / rebase-merge / rebase-apply present ──► PreconditionFailed
                                                          (message lists which)
managed_by_app == false ──► forget the workspace only; disk untouched
managed_by_app == true  ──► (force: kill its sessions)
                            git worktree remove [--force] -- <path>
                            git worktree prune
```

`precheck_remove` resolves git paths with `rev-parse --git-path`, so linked
worktree git dirs are handled. The GUI is expected to call without `force`,
show the precondition message, and re-issue with `force = true` after the
user confirms.

A worktree directory **deleted outside the app** is a normal input to this
path, not an error. `precheck_remove` reports nothing (there is no working tree
to be dirty and no sequencer state to be mid-replay) and the removal reduces to
`git worktree prune`, which drops the administrative entry that is all that is
left of it. Both used to fail — `git status` cannot run in a directory that is
gone, and `git worktree remove` refuses a path that is not a working tree — so
the workspace was stuck in the rail with no action that could clear it.

The same situation is also repaired without the user asking:
`RefreshWorkspaceStatus` on a workspace whose path no longer exists re-runs the
§15.3 step 4 diff for its project instead of silently answering `Ack`. Before
that, step 4 ran only at daemon start and from `RefreshProject`, so a worktree
removed in a terminal kept its row — and its status dot — until the next
restart.

`RemoveProject` with `KillSessionsRemoveManagedWorktrees` applies the managed
branch of the table to every managed worktree of the project (always forced,
since sessions were just killed). See
[protocol.md](./protocol.md#removeprojectpolicy).

## Git errors

`GitError`: `NotAGitRepo(path)`, `InvalidBranchName { branch, reason }`, `Conflict { branch, path }`,
`CommandFailed { args, status, stderr }`, `Timeout`, `Io`, `NonUtf8Path`.
The daemon (`core.rs::git_err`) maps `Conflict` → `ErrorCode::Conflict`,
`InvalidBranchName` → `ErrorCode::InvalidRequest`, and everything else →
`ErrorCode::GitError`.

## Tests

- `crates/git-service/tests/integration.rs` creates real temp repositories;
  it **returns early when `git` is absent**, so a green run on a machine
  without Git proves nothing.
- `crates/daemon/tests/integration.rs::worktree_lifecycle` goes
  through the protocol end to end and fails (not skips) without Git.
- `crates/daemon/tests/scenario_branches.rs` covers the branch/remote half:
  listing, the "already checked out" answer, the asynchronous fetch and its
  failure path, tracking a remote branch, `RefreshProject` adopting a worktree
  made outside the app, and provisioning. `origin` is a second temporary
  repository reached over a local path — a first-class git transport — so the
  fetch path is the real one and nothing touches the network.
