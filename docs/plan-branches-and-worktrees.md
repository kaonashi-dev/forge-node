# Implementation plan — Branches, remotes and workspace switching (§14 + §16)

**Status:** **implemented** (2026-08-24). See the record in [`execution.md`](../execution.md).
The open decisions were closed with the recommended option of each one: worktree-first without
`SwitchBranch` (D1-A), fetch when the dialog opens and non-blocking (D2-2), and the worktree under
`worktrees.root` with `copy` + `setup_script` (D3-1). What is left out is in §6 *Later*.
**Date:** 2026-08-24
**Scope:** make creating, choosing and switching branches a GUI operation —
including branches that only exist on the remote — without breaking ADR-008 nor
the per-worktree isolation (P4).

---

## 0. Research findings (the real code, not `plan.md`)

The worktree backend is **complete and tested**. What is missing is everything
that touches remote branches and, above all, **the GUI**.

| Piece | Location | Status |
|-------|----------|--------|
| `Request::CreateWorktree { project_id, branch, base, name }` | `protocol/src/request.rs:160` | Complete |
| `Request::RemoveWorktree { workspace_id, force }` | `protocol/src/request.rs:171` | Complete |
| `Request::RefreshWorkspaceStatus` | `protocol/src/request.rs:180` | Complete |
| `git_service::create` / `remove` / `precheck_remove` | `git-service/src/worktree.rs:166` | Complete, with real tests |
| `Daemon::create_managed_worktree` | `daemon/src/core.rs:1229` | Complete; already factored out for reuse |
| `Daemon::rescan_worktrees` | `daemon/src/core.rs:384` | Complete, **only in `Daemon::start`** |
| `list_branches` | `git-service/src/repository.rs:113` | **Local branches only** (`branch --list`, no `-r`/`--all`) |
| `default_branch` | `git-service/src/repository.rs:85` | Reads `refs/remotes/origin/HEAD`; it may not exist |
| `RepoStatus { dirty, ahead, behind }` | `git-service/src/repository.rs:12` | It is computed, **the protocol does not carry it** |
| `RuntimeCommand` (the GUI) | `ui/src/runtime.rs:25-99` | **No worktree and no branch variant** |
| `SidebarAction` | `ui/src/sidebar.rs:37-45` | Groups and projects; **nothing about workspaces** |
| `PaletteAction` | `apps/tauri/src/palette/` | Sessions and settings; **it does not list workspaces** |

Five consequences you only see by reading the code:

1. **The GUI cannot create or delete a worktree.** `CreateWorktree` has existed in
   the protocol and the daemon for months, but there is not a single path from
   the window down to it: `RuntimeCommand` has no such variant. `sidebar.rs:489`
   only *paints* the workspaces that already exist. Every worktree the user sees
   today was created by hand and discovered by `rescan_worktrees` when the daemon
   started.

2. **`fetch` does not exist in the whole workspace.** `grep -rn "fetch"
   crates/git-service` returns nothing. Without it, "pull a branch from the
   remote" is literally impossible from the app: `list_branches` only sees
   `refs/heads`, so a branch that only lives in `origin` **does not even show up
   in the list**.

3. **But `create` already knows how, by a fortunate accident.** With
   `branch = "feature/x"` (non-existent locally) and
   `base = Some("origin/feature/x")`, `worktree.rs:166` falls into the
   `worktree add -b <branch> -- <path> <base>` branch, and since the starting
   point is a remote-tracking branch, git's default `branch.autoSetupMerge`
   **configures the upstream on its own**. Neither `--track` nor a signature
   change is needed: what is needed is *knowing* that `origin/feature/x` exists,
   and for that you need `fetch` + listing the remotes.

4. **`RefreshProject` lies.** `core.rs:1153` only re-runs `discover_root` and
   updates `git_root`; it neither re-reads the branch nor re-scans worktrees. A
   worktree created outside the app does not show up until the daemon restarts.
   It is already documented in `docs/worktrees.md`, and this feature makes it far
   more visible.

5. **Switching branches *inside* a workspace is not a Forge operation.** There is
   no `checkout` and no `switch` in any crate. Today "switching branches" can
   only mean "switching workspaces", and that only happens by clicking another
   row in the sidebar.

---

## 1. The problem

Two different things the user merges into one sentence, and that are worth
separating:

**(A) Starting to work on a branch that exists on the remote.**
Today: impossible from the app. The user has to go to a terminal, `git fetch`,
`git worktree add`, and wait for the daemon's next start (or add the project
again) for Forge to see it. It is exactly the flow the app exists to eliminate.

**(B) Moving quickly between the branches they are already working on.**
Today: half there. The sidebar lists the workspaces and one click changes
context, but there is no shortcut, there is no search, the command palette does
not list them, and the row says nothing useful (no *dirty*, no *ahead/behind*,
no count of live agents when it is expanded).

And an implicit third one, which is what makes (A) and (B) worth it: **firing an
agent on a specific branch in one step**. Today that is three: create the
worktree (outside the app) → restart the daemon → launch the agent.

---

## 2. How others solve it

The three relevant reference points converge on the same model, and it is the one
`plan.md` already picks in P4:

- **[Conductor](https://www.conductor.build/docs/concepts/workspaces-and-branches)**
  — one worktree per agent, under `~/conductor/workspaces/<repo>/<workspace>`.
  `⌘⇧N` opens "New workspace" with a **"Branches" tab** to pick an existing
  branch instead of creating one. **It runs `git fetch origin` before creating**,
  so a new workspace always starts from the latest remote commit even if the
  local checkout is behind. The base branch is configurable per repository
  (`origin/main`). When the branch is already *checked out* in another workspace,
  it does not try to be clever: it tells you and offers to create a variant
  (`scroll-to-bottom-btn-2`). It also copies allowed local files (`.env`) and
  runs *setup scripts* in the new worktree.
- **[Crystal](https://nimbalyst.com/blog/best-git-worktree-tools-ai-coding-2026/)**
  — parallel Claude Code / Codex sessions, each in its own worktree, with
  integrated git operations and a diff panel.
- **[Vibe Kanban](https://virtuslab.com/blog/ai/vibe-kanban)** — every card on the
  board *is* a worktree; isolation is the unit of work, not an option.

The cross-cutting observation of [Nimbalyst's comparison](https://nimbalyst.com/blog/best-git-worktree-tools-ai-coding-2026/)
is the warning that affects us most: *git does not allow the same branch in two
worktrees*, so the tools either force unique names per session (`session/<id>`)
or run in detached HEAD. Forge already returns `GitError::Conflict` with the path
of the offending worktree (`worktree.rs:181-190`) — what is missing is for the
GUI to tell it well.

**Conclusion:** nobody offers "switch branch in-place" as the main action. In a
worktree-first model, *switching branches is switching workspaces*. That must be
the main path here too.

---

## 3. Proposed model

A single product idea: **"New workspace…" is the dialog where everything to do
with branches lives.** A finder that merges four sources into one list, in the
style of the command palette that already exists:

```
┌─ New workspace — forge-node ─────────────────────────┐
│ 🔍 feat/remo                                          │
├───────────────────────────────────────────────────────┤
│ CREATE                                                │
│  ✚ feat/remo                     from origin/main     │
│ LOCAL BRANCHES                                        │
│  ⑂ feat/remote-branches          ↑2  ⚠ in ~/…/wt-3    │  ← taken
│ REMOTE BRANCHES                                       │
│  ☁ origin/feat/remote-tabs       2h ago               │
│  ☁ origin/feat/remove-legacy     3d ago               │
├───────────────────────────────────────────────────────┤
│ ⟳ fetched 12s ago                          ⏎ Create   │
└───────────────────────────────────────────────────────┘
```

- **CREATE** appears whenever what is typed is not an existing branch: it creates
  a new branch from the project's base branch (`origin/HEAD`, or `HEAD` if there
  is no remote).
- **LOCAL** and **REMOTE** are `branch --list` and `branch --list -r`, sorted by
  `committerdate` descending. A local branch already *checked out* in another
  workspace is shown **disabled with the path that holds it** — the
  `GitError::Conflict` turned into UI, before trying.
- Picking a remote one creates the worktree with `branch = <branch without
  origin/>`, `base = origin/<branch>`: upstream configured for free (finding 3).
- Opening the dialog fires a `fetch --prune` **in the background**; the list is
  painted instantly with what is already there and repainted when the fetch
  finishes. The footer says when the last one was. It never blocks.

And for (B), three small things:

- The **command palette** lists the active project's workspaces
  (`PaletteAction::FocusWorkspace`) — it is the direct answer to "let me switch
  easily between those branches".
- Shortcut **`⌘⇧N`** → New workspace (the same as Conductor, and `⌘⇧N` is free).
- **A context menu on the workspace row**: *New worktree…*, *Remove worktree…*,
  *Fetch*, *Copy path*, *Reveal in Finder*. Today `workspace_row`
  (`sidebar.rs:489`) has none.

---

## 4. Blockers

Ordered by how expensive they get if discovered late.

### B1 — The GUI loop is single-threaded and the queue is shared (critical)

`runtime_loop` (`ui/src/runtime.rs:157`) drains **a single channel** carrying both
`RuntimeCommand::Input` (the keystrokes!) and every mutation, and every mutation
is a **blocking** `client.request()`. No current command takes more than a few
milliseconds, so up to today it has not mattered.

A `fetch` takes between 300 ms and tens of seconds. If it travels through that
queue, **typing in the terminal freezes while it lasts**. It is the first network
command to enter that path and it breaks the premise it is built on.

*Way out:* network operations do not travel through `RuntimeCommand`. Either a
second thread with its own `Client` (the daemon already accepts several
connections), or — better — the daemon does them **asynchronously**:
`Request::FetchRemote` answers `Ack` immediately and the result arrives as
`DaemonEvent::RemoteRefsUpdated`. That fits "mutations return `Ack`, changes
arrive by broadcast" (AGENTS.md) and leaves the GUI reactive without touching the
loop.

### B2 — Credentials: `GIT_TERMINAL_PROMPT=0` does not cover SSH

`run_git` (`command.rs:126-128`) forces `LC_ALL=C` and `GIT_TERMINAL_PROMPT=0`.
That stops HTTPS from asking for a username/password, **but `ssh` does not read
that variable**: a key with a passphrase and no agent loaded, or an unknown host,
leaves `ssh` waiting on the TTY until the timeout. Since the daemon runs without
a terminal, in practice it fails — but it can hang 30 s before doing so.

*Way out:* for network commands, add
`GIT_SSH_COMMAND="ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new"` and
`GIT_ASKPASS`/`SSH_ASKPASS` pointing at a binary that fails fast. A credentials
failure must reach the UI as *"I could not authenticate against origin"*, with
the real stderr one click away, not as a mute timeout.

### B3 — ADR-008's 30 s timeout is a decision about *local* commands

`GIT_TIMEOUT` (`command.rs:22`) is a single constant for everything. An initial
`fetch` of a big monorepo goes past 30 s routinely; a `rev-parse` that takes 30 s
is a bug. A single number cannot serve both.

*Way out:* split out `run_git_network` with its own timeout (120 s by default,
configurable in `[git]`). It is a **deviation from ADR-008** and it has to be
written down as such in `docs/worktrees.md` or in a new ADR, not smuggled in.

### B4 — The worktree lives far from the repository

`worktrees.root` is
`~/Library/Application Support/Forge/worktrees/<project-id>/<slug>`
(`core.rs:1260`). Right for not dirtying the repo, but it means the new worktree
**does not have** `.env`, `node_modules`, `.venv`, `target/`, nor any relative
symlink. An agent launched there fails on the first command and the user blames
Forge.

Conductor solves this explicitly: it copies allowed local files and runs *setup
scripts*. We have neither.

*Way out:* it is outside the minimal scope, but **it has to be decided before
showing the button**, because it defines whether the feature feels finished. The
cheapest thing that works: `[worktrees] copy = [".env", ".env.local"]` and a
`setup_script`, run after `worktree add`. An even cheaper alternative: allow the
worktree to be created **next to the repo** (`../<repo>-<slug>`), which is where
the JS/Python tooling expects to find things.

### B5 — The rescan only happens when the daemon starts

With this feature the user is going to create worktrees from the app *and* from
the terminal, and is going to expect both to show up. `RefreshProject`
(`core.rs:1153`) does not re-scan. It has to be fixed — it is three lines
(`reconcile_project_worktrees` already exists and already emits the broadcasts,
`core.rs:429`) and without it the feature looks broken half of the time.

### B6 — The protocol does not carry the repo state

`RepoStatus` has `dirty`, `ahead` and `behind`, and `RefreshWorkspaceStatus`
computes them — but `Workspace` (`domain/src/workspace.rs:22`) only carries
`branch`, so they are thrown away. A branch list with no "↑2 ↓5 ●" is half as
useful.

*Way out:* add the three fields to `Workspace` as **runtime state, not columns** —
exactly the pattern `Session::terminal_id` and `last_activity_at` already use
(AGENTS.md). No SQLite migration, `serde(default)` so an old client does not
break.

### B7 — Git details that bite in real repos

- `origin/HEAD` **does not exist** in a clone made with `--single-branch` nor
  after some older `git clone`s → `default_branch` returns `None`. Fallback:
  `main`, `master`, or plain `HEAD`.
- **Remote-less** repos (local projects): the dialog must hide the REMOTE section
  and the whole fetch, not fail.
- **Bare** repos and **non-git** projects: `Project::is_git()` already covers it;
  the button is disabled.
- **Multiple remotes** (`origin` + `upstream`): v1 assumes `origin`. An explicit,
  written decision, not an accidental one.
- **Submodules**: `worktree add` does not initialise them. Out of scope, but it
  deserves a line in the docs.
- `git fetch --prune` may **delete** the tracking branch of a worktree the user
  has open. Nothing breaks (the worktree stays on its local branch), but the UI
  must not suddenly show "branch gone".

---

## 5. Alternatives (what has to be decided)

### D1 — Worktree-first, or in-place checkout too?

| | **A. Worktree only** *(recommended)* | **B. `SwitchBranch` too** | **C. Hybrid** |
|---|---|---|---|
| What it is | Switching branch = creating/choosing another workspace | `git switch` inside an existing workspace | A as the main path, B only in the `Main` checkout and with a clean tree |
| For | It is P4 and it is what Conductor, Crystal and Vibe Kanban do. Zero risk for live sessions | Cheap (`git switch` and one event). It is what people already have in their fingers | Covers the "I just want to look at something on main for a second" case without opening a worktree |
| Against | One worktree per branch costs disk and needs B4 solved | **It changes the ground under live agents**: their `cwd` stays there, their open files change. It demands a clean tree and zero active sessions | Two paths to explain |
| Cost | — | +`Request::SwitchBranch`, its own prechecks | A + B |

Recommendation: **A** for v1, and **C** if usage asks for it. B as the main action
contradicts the product's thesis.

### D2 — When does `fetch` happen?

| | **1. Manual** | **2. When the dialog opens** *(recommended)* | **3. Periodic in the background** |
|---|---|---|---|
| For | Zero surprises, zero unrequested network | The list is fresh when it matters. It is what Conductor does | Everything always up to date |
| Against | The user does not know they have to press it → they see an old list and conclude it does not work | A small delay the first time | Constant unrequested network; battery; private repos with broken auth failing in a loop |

Recommendation: **2**, non-blocking, with **1**'s manual button always visible and
the "12 s ago" mark. **3** behind a `[git] auto_fetch_secs = 0` (disabled by
default), never as implicit behaviour.

### D3 — Where does the worktree live? (see B4)

| | **1. `worktrees.root`** (today) | **2. Next to the repo** `../<repo>-<slug>` | **3. Configurable per project** *(recommended)* |
|---|---|---|---|
| For | Does not dirty the repo; centralised, safe deletion | `.env`, `node_modules` and relative paths keep working | Each repo picks |
| Against | Every relative environment breaks → it forces copy/setup | Dirties the parent directory; the user sees them in their file browser | One more field in `Project` (+ migration) |

Recommendation: keep **1** as the default and add **copy + setup_script**, which
is what actually solves the problem; leave **3** for when there is a real
request.

### D4 — How does the fetch result reach the GUI? (see B1)

Recommended: **`Request::FetchRemote` → immediate `Ack` +
`DaemonEvent::RemoteRefsUpdated`**. The alternative (a blocking request on a
second client thread) is less code but leaves the result outside the `Store`, so
a second connected client never finds out. The broadcast pattern is already the
project's.

---

## 6. Phased plan

Every phase leaves the tree green (`scripts/dev check`) and is useful on its own.

### Phase 1 — The GUI can create and delete worktrees *(no network)*

What makes a backend written months ago usable. **No protocol change.**

- `ui/src/runtime.rs`: `RuntimeCommand::{CreateWorktree, RemoveWorktree, RefreshWorkspaceStatus}`.
- `ui/src/dialogs.rs`: `Dialog::NewWorktree` (branch name + base + optional name)
  and `Dialog::ConfirmRemoveWorktree` (which knows how to read the
  `PreconditionFailed` and re-emit with `force`).
- `ui/src/sidebar.rs`: a `+` button on the project row → *New worktree…*; a
  context menu on `workspace_row` (today it has none).
- `client`: wrappers, in the line of `set_app_state`.

*Test:* with real Git, create a worktree from the app → it shows up in the
sidebar and can launch an agent. Delete it dirty → precondition → confirm → it
goes away.

### Phase 2 — `RefreshProject` really re-scans (B5)

`core.rs:1153` calls `reconcile_project_worktrees` and re-reads the branch. Test:
`git worktree add` from outside → `RefreshProject` → `WorkspaceCreated` arrives.
Ten lines, and without it Phase 1 looks intermittent.

### Phase 3 — Branches: really list them

- `git-service/src/repository.rs`: a `list_refs()` returning
  `Vec<BranchRef { name, remote: Option<String>, upstream, committed_at, head }>`
  through `for-each-ref --sort=-committerdate --format=…` over `refs/heads` and
  `refs/remotes`. It replaces `list_branches`, which remains as a special case.
- `protocol`: `Request::ListBranches { project_id }` →
  `Response::Branches(Vec<BranchRef>)`.
- `ui`: the Phase 1 dialog becomes the §3 finder, **without the REMOTE section
  yet** (locals only) and already with the "taken by another worktree" marking.

### Phase 4 — Network: fetch (B1, B2, B3)

- `git-service/src/command.rs`: `run_git_network` with its own timeout, SSH
  `BatchMode` and askpass neutralised. Document the deviation from ADR-008.
- `git-service/src/remote.rs`: `fetch(repo, remote, prune)`, `list_remotes(repo)`.
- `daemon`: `Request::FetchRemote { project_id }` → immediate `Ack`; the fetch
  runs in its own thread; when it finishes it emits
  `DaemonEvent::RemoteRefsUpdated { project_id, error: Option<String> }`. One
  fetch in flight per project (coalesce, do not queue).
- `ui`: the REMOTE section in the dialog; fetch on open; a footer with "N s ago"
  and a manual button; the auth error is shown in the footer, with stderr one
  click away.
- Picking a remote one →
  `CreateWorktree { branch: "feat/x", base: Some("origin/feat/x") }`, which
  already works (finding 3). A test that the upstream ends up configured.

### Phase 5 — Switching workspace fast (problem B)

- `PaletteAction::FocusWorkspace` + workspace entries in the command palette.
- Shortcut `⌘⇧N` → New workspace.
- `Workspace` gains `dirty`/`ahead`/`behind` as runtime state (B6) and the sidebar
  paints them.

### Phase 6 — Making the new worktree useful (B4)

`[worktrees] copy = [...]` and `setup_script`, run after `worktree add` with
their output visible. It is what separates "it creates a directory" from "it
creates a working environment".

### Later (not in this plan)

- `ChildWorkspacePolicy::NewManagedWorktree`, rejected today in `core.rs:1595`
  even though `create_managed_worktree` is already factored out to serve it:
  *"launch this agent on a new branch"* in a single step.
- `RemoveWorktree` prechecks structured in the protocol instead of text (already
  listed as pending in `execution.md`).
- Push / PR / diff. That is another product; it is worth saying explicitly that
  it is not included.

---

## 7. Scope risks

- **The feature looks small and it is not.** The backend exists, yes, but what
  makes it useful (network, credentials, the worktree's environment, reactivity)
  is exactly what does not exist. Phases 1–3 are a weekend; phase 4 is the one
  with real unknowns.
- **B1 cannot be postponed.** The first `fetch` that travels through
  `RuntimeCommand` will freeze typing, and the bug will read as "the terminal
  sometimes hangs", which is extremely expensive to diagnose. It gets decided in
  Phase 4 or it gets paid for later.
- **Without Phase 6, Phase 4 disappoints.** A freshly created worktree where
  `npm test` fails is not a finished feature, and the user is not going to blame
  the configuration.
