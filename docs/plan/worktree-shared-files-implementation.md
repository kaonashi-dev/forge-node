# Plan — Shared files between worktrees (implementation)

Status: **plan only.** No code in this document has been applied.
Alternatives, prior art and the reasoning behind every choice:
[worktree-shared-files.md](./worktree-shared-files.md). This document assumes
those decisions and only says *what to build, where, in what order, and how it
is verified*.

Related: [worktrees.md](../worktrees.md) §14,
[plan-branches-and-worktrees.md](../plan-branches-and-worktrees.md) §B4,
[protocol.md](../protocol.md), [persistence.md](../persistence.md),
`AGENTS.md` § *Boundaries And Invariants*, `harness/CHECKPOINTS.md` C4/C5/C7.

---

## 0. Decisions taken

| # | Decision | Choice |
|---|---|---|
| D1 | Strategy set in v1 | `copy` + `clone` + `run` + `link`. `env` (shared caches through `CARGO_TARGET_DIR` & friends) is **out of v1**, designed for but not built. |
| D2 | Source of truth for `link` | A per-project store at `$GIT_COMMON_DIR/forge/shared/`, with an explicit, backed-up, reversible migration of the real file into it. |
| D3 | Where rules live | Per project in SQLite, edited in the GUI, carried over the protocol. `[worktrees] copy` / `setup_script` stay as the **global default layer** for projects with no rules of their own. |
| D4 | When it runs | On create, on adopt, and on demand. **No** `post-checkout` git hook in v1. |
| D5 | Directories | Allowed for `clone`; allowed for `copy` only past an explicit size confirmation. |
| D6 | Secrets | Classified, warned about once when a strategy duplicates them at rest, never printed in notices or logs. |
| D7 | Ignore hygiene & status | Managed block in `$GIT_COMMON_DIR/info/exclude`; per-workspace share status is computed and shown. |
| D8 | Scope | Rules apply to **every workspace of the project**, main checkout included (P4). |

## 1. What the user gets

1. A **Shared files** section per project in Settings. It lists what the project
   ignores (`.env`, `node_modules/`, `.venv/`, `target/`, …), each row with a
   strategy picker and an explanation of what that strategy means here — and any
   other path the user names, added with a picker or typed by hand, whether or
   not the scan proposed it (§2.4).
2. A worktree created from Forge arrives with those files **already in place**,
   and says so while it is doing it instead of freezing the UI.
3. A worktree created in a terminal and adopted by the rescan gets the same
   treatment.
4. An existing worktree can be re-synced from the workspace row or the section,
   with a **preview** of what would change before anything is written.
5. The list is theirs: a rule can be added, re-pointed at another strategy,
   disabled, reordered or removed, and removing one asks once what to do with
   the files it already put in each worktree.
6. Every workspace shows, per rule, whether the share is `applied`, `missing`,
   `severed` (a link that a tool replaced with a real file), `diverged`,
   `skipped` or `failed`.

## 2. The model

### 2.1 Domain types — new `crates/domain/src/share.rs`

`ShareRuleId` goes in `domain/src/ids.rs` next to the others: `uuid_id!` is a
plain `macro_rules!` there with no `#[macro_export]`, so it is not reachable
from a sibling module.

```rust
/// How one path becomes available in a workspace (§14.2, plan §5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ShareStrategy {
    /// Byte-for-byte copy from the source workspace. Total isolation.
    Copy,
    /// Copy-on-write clone (APFS `clonefile`, Linux `FICLONE`), falling back to
    /// `Copy` with a reported `fallback` flag. Directories allowed.
    Clone,
    /// A symlink into the project's shared store. One file, every workspace.
    Link,
    /// Produce the path by running a command in the workspace, when it is
    /// missing. The path is the goal; the command is how it is reached.
    Run { command: String, timeout_secs: u64 },
}

/// One rule of one project (§14.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRule {
    pub id: ShareRuleId,
    pub project_id: ProjectId,
    /// Relative to the repository root. No `..`, no absolute, never inside
    /// `.git`. A trailing `/` is not part of the identity: `node_modules` and
    /// `node_modules/` are the same rule.
    pub path: String,
    pub strategy: ShareStrategy,
    /// Off keeps the row (and its history) without applying it.
    pub enabled: bool,
    /// Application order. `Run` rules always sort last, whatever this says.
    pub position: u32,
    pub created_at: Timestamp,
}

/// What the classifier thinks a path is, which is what drives the default
/// strategy the GUI proposes. Never used to *decide* anything at apply time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ShareClass { Secret, Dependencies, BuildOutput, Cache, EditorState, Other }

/// A path Forge found ignored in the project and can propose a rule for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareCandidate {
    pub path: String,
    pub class: ShareClass,
    pub is_dir: bool,
    /// `None` when the scan hit its budget before finishing the subtree.
    pub size_bytes: Option<u64>,
    pub entries: Option<u64>,
    pub suggested: ShareStrategy,
    /// A rule for this path already exists.
    pub already_ruled: bool,
}

/// Runtime-only, like `Workspace::status`: never a column, never persisted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ShareState {
    Applied,
    Missing,
    /// The link Forge wrote is now a regular file (an editor's `rename()`).
    Severed,
    /// A copy/clone whose source has changed since it was made.
    Diverged,
    Skipped { reason: String },
    Failed { message: String },
}

pub struct ShareStatusEntry { pub rule_id: ShareRuleId, pub path: String, pub state: ShareState }

/// One line of a preview or of an application result.
pub struct ShareAction {
    pub rule_id: ShareRuleId,
    pub path: String,
    pub strategy: ShareStrategy,
    pub verb: ShareVerb,            // Copy | Clone | Link | Run | Skip | Backup
    pub bytes: Option<u64>,
    /// Set when `Clone` will degrade to a full copy on this filesystem.
    pub fallback: bool,
    pub note: Option<String>,
}
```

`ShareState`, `ShareCandidate`, `ShareAction` are **runtime-only** — no column,
no migration, no `client::Store` field (the same rule as `WorkspaceDiff`,
`RebaseState` and `FileTree`). `ShareRule` is the only persisted one.

### 2.2 The store on disk

```
$GIT_COMMON_DIR/forge/
├── shared/                  # real files, one per `link` rule, path-shaped
│   ├── .env
│   └── certs/local.pem
└── backups/                 # what a migration displaced, never deleted by us
    └── 2026-09-04T12-03-11Z/.env
```

- Resolved with `git rev-parse --git-common-dir` from *any* workspace, so it is
  the same directory for the main checkout and every worktree (§3.2 of the
  alternatives doc). Created `0700` through `paths::ensure_private_dir`.
- Activating a `link` rule for a path that has a real file in the source
  workspace **moves** it into `shared/`, writes a symlink in its place, and
  copies the original into `backups/<timestamp>/`. This is the only intrusive
  operation in the feature and it never happens without an explicit confirm.
- Removing a rule (or a worktree) removes links, never the store entry.
  A "Stop sharing and keep a copy here" action materialises the store file back
  into the workspace.

### 2.3 Precedence

```
built-in defaults  →  [worktrees] copy / setup_script (global)  →  project rules (DB)
```

A project with **no** rules behaves exactly as today, so nothing changes for an
existing user until they open the section. A project with rules ignores the
global `copy` list entirely — merging two lists silently would make it
impossible to *remove* an inherited entry. The section says which layer is in
effect, and offers **"Import the global list as rules"** as a one-click start.

### 2.4 The list is edited, not only detected

Detection (§F3) exists to make the first rule cheap, not to define what can be
shared. Anything the user names is a valid rule, and the list is theirs to
change afterwards.

**Where a shared file comes from.** The source is the project's **`Main`
workspace** — the primary checkout, `WorkspaceKind::Main`, which is what
`copy_into_worktree(git_root, …)` already uses today (`core.rs:4091`). It is
"the main branch" in the sense that matters here: the checkout the user works
in and keeps their real `.env` in, whatever branch it happens to have out.
`link` rules are the exception — their source is the store (§2.2), which is why
adopting a path into the store is its own explicit request.

A path that does not exist in the `Main` workspace is **not an error**: the rule
is stored and reported as `Skipped { reason: "not present in the source" }`
until the file appears. A rule can therefore be written *before* the file
exists, which is what happens when someone sets the project up for a teammate.

**Adding a path.** Three entry points, all producing the same `ShareRule`:

1. **From the detected list** — the `[Add ▾]` on a candidate row, with the
   classifier's suggested strategy preselected.
2. **From the OS picker** — *Add a file…* opens the native picker rooted at the
   `Main` workspace. The chosen absolute path is made relative to the repository
   root; a path **outside** the repository is refused with that reason, not
   silently rewritten.
3. **By typing or pasting** a relative path, for a file that does not exist yet
   or lives deep enough that browsing is slower than typing.

**Validation at the moment of adding**, before the rule is stored:

| Case | Answer |
|---|---|
| absolute, `..`, inside `.git`, escapes the repo | refused, with the reason (`apply.rs::validate`, one implementation for the GUI and the daemon) |
| already has a rule | the existing row is focused instead of a duplicate created (the unique index of §F2 is the backstop, not the UX) |
| **tracked by git** (`git ls-files --error-unmatch`) | refused: git already carries it into every worktree. Sharing it would put an untracked copy on top of a tracked file. |
| untracked and **not ignored** | allowed, with an offer to add it to the managed `info/exclude` block (§F3) — otherwise the injected copy makes every worktree dirty |
| a directory, with `copy` selected, over the budget | allowed after the D5 size confirmation |

**Changing a rule.** The strategy of an existing rule is editable in place. A
change is **not retroactive**: workspaces keep what they already have until the
next apply, and the section marks the changed rows as *pending* with a
**Re-apply** action. Silently rewriting files in a checkout an agent is using is
exactly what §M9 was rejected for.

**Disabling vs removing.** `enabled = false` keeps the row and its position and
stops applying it, leaving what is already on disk untouched — the way to stop
sharing a path without deciding what to do with the copies. Removing the row
asks that question once:

```rust
pub enum ShareCleanup {
    /// Leave every workspace exactly as it is. The default.
    Leave,
    /// Delete what Forge put there — only paths whose current state matches
    /// what the rule wrote (a link into the store, or a copy Forge made).
    /// Anything modified since is left and reported.
    RemoveInjected,
    /// `link` only: replace each workspace's symlink with a real copy of the
    /// store file, so every checkout keeps a working, independent file.
    Materialize,
}
```

`RemoveInjected` never deletes a path it cannot prove it created: a `copy` whose
content changed since it was written is `Skipped`, and the removal says so.
Deleting an agent's edited `.env` because a rule was removed is not recoverable
and would be our fault.

**Removing the store entry** is a separate, explicit gesture (§2.2): dropping a
`link` rule leaves `forge/shared/<path>` in place, so re-adding it later finds
the file rather than an empty store.

## 3. Where the code goes (C4)

| Concern | Crate / module | Why there |
|---|---|---|
| `ShareRule`, `ShareStrategy`, `ShareState`, `ShareCandidate`, `ShareAction` | `domain/src/share.rs` (new) | shared serializable state |
| Requests / responses / events | `protocol/src/{request,response,event}.rs` | transport-independent messages |
| Typed client calls | `client/src/ipc.rs`, rule cache in `client/src/store.rs` | GUI reaches wire types through `client` only |
| `git rev-parse --git-common-dir`, `info/exclude` block | `git-service/src/repository.rs` | every application git subprocess goes through `run_git` (ADR-008) |
| Candidate discovery (`status --ignored`) | `git-service/src/repository.rs` + classification in `daemon/src/shares/detect.rs` | the subprocess is git's; the taxonomy is ours |
| Planning: rules + observed disk → `Vec<ShareAction>` | `daemon/src/shares/plan.rs` — **pure**, no I/O | the `idle.rs` precedent: policy is a pure function, effects live in `core.rs` |
| Mechanisms: copy / clone / link / run, path validation | `daemon/src/shares/apply.rs` | filesystem effects, off the core lock |
| Orchestration: which trigger, which workspace, notices, events | `daemon/src/core.rs` | the one place that owns `Inner` |
| Rules table | `persistence/src/{migrations.rs,repositories/shares.rs}` | metadata only |
| Worker plumbing | `apps/tauri/src-tauri/src/runtime/workbench.rs` | slow reads/writes never share the channel that carries keystrokes |
| Section, dialog summary, badges | `apps/tauri/src/settings/SharedFiles.tsx`, `panels/…` | theme tokens only, no hardcoded colors |

`fs-service` is deliberately **not** used: it exists to answer GUI reads inside a
checkout (ADR-012) and provisioning is a daemon-side write. Nothing new depends
on `terminal-core`; nothing in the GUI does `std::fs`.

## 4. The blocking-work finding (this shapes the design)

`provision_worktree` (`crates/daemon/src/core.rs:2174`) runs **inline inside the
`CreateWorktree` request** (`core.rs:2145`), before the workspace row exists.
With `setup_script = "npm ci"` that is up to `setup_timeout_secs` (default 120 s)
of subprocess inside a synchronous request. The GUI drains one command channel on
one thread and **that channel also carries `RuntimeCommand::Input`** — the same
reason `FetchRemote` and `CreatePullRequest` ack-then-event (AGENTS.md). So
today, creating a worktree in a project with a setup script can freeze typing for
two minutes.

v1 fixes that as part of the work:

- `CreateWorktree` **stops provisioning inline**. It creates the checkout,
  persists the workspace, broadcasts `WorkspaceCreated`, and then *starts*
  provisioning on a worker.
- Provisioning acks when it starts and reports with
  `DaemonEvent::SharesApplied { workspace_id, actions, error }`.
- It is **coalesced, not queued**, per workspace: `Inner::provisioning:
  HashSet<WorkspaceId>`, cleared by a `Drop` guard (`Daemon::lock` recovers from
  poisoning, so a panicked worker must not latch the flag — the
  `fetching`/`pr_opening` precedent).
- The core lock is never held across a copy, a clone, a link or a script:
  the worker takes the lock to read the rules, drops it, does the I/O, takes it
  again to broadcast.
- The new worktree is visible immediately with a *setting up…* state, which is
  strictly better than a two-minute freeze followed by a directory appearing.

Test impact: `crates/daemon/tests/scenario_branches.rs`'s provisioning assertion
becomes "wait for `SharesApplied`, then assert the file", not "assert right after
the `CreateWorktree` reply".

## 5. Work, in dependency order

Each block is one harness feature (`scripts/harness from-issue` → `/feature`),
sized to be reviewable on its own. F1–F4 are daemon-side and land in order;
F5–F6 are GUI and can start once F1 is merged.

### F1 — Domain, protocol, client

**Files:** `crates/domain/src/share.rs` (new), `domain/src/lib.rs`,
`domain/src/ids.rs` (the `ShareRuleId` declaration), `protocol/src/request.rs`, `protocol/src/response.rs`,
`protocol/src/event.rs`, `client/src/ipc.rs`, `client/src/store.rs`.

Requests:

```rust
/// Read the ignored paths a project could share (§14.2). Local, synchronous,
/// bounded — like `ListBranches`, it never opens a socket.
DetectShareCandidates { project_id: ProjectId },
/// Replace a project's rule set wholesale → `Ack`; `ProjectSharesChanged`.
SetProjectShares { project_id: ProjectId, rules: Vec<ShareRule> },
/// What applying the rules to this workspace would do. Writes nothing.
PreviewShares { workspace_id: WorkspaceId },
/// Per-rule state of one workspace. One `lstat` per rule, no subprocess.
GetShareStatus { workspace_id: WorkspaceId },
/// Apply the rules → `Ack` when the work *starts*; `SharesApplied` carries the
/// outcome. Coalesced per workspace.
ApplyShares { workspace_id: WorkspaceId, only: Option<Vec<ShareRuleId>> },
/// Move a real file into the shared store and leave a link (the D2 migration).
/// Explicit, separate request: it is the only operation that touches the user's
/// primary checkout.
AdoptIntoShareStore { project_id: ProjectId, path: String },
/// The inverse: copy the store file back into a workspace as a real file.
/// Used by `ShareCleanup::Materialize` and by "stop sharing, keep a copy".
MaterializeFromShareStore { project_id: ProjectId, path: String, workspace_id: Option<WorkspaceId> },
/// Drop one rule and say what happens to what it already put on disk (§2.4).
/// Separate from `SetProjectShares` because it has effects outside the row.
RemoveShareRule { project_id: ProjectId, rule_id: ShareRuleId, cleanup: ShareCleanup },
```

`SetProjectShares` carries the whole set, so adding, reordering, enabling and
editing a strategy are all one request and one event — the GUI edits a list and
saves it. Only **removal** is its own request, because it is the only edit that
can delete a file.

Responses: `ShareCandidates { candidates }`, `SharePlan { workspace_id, actions }`,
`ShareStatus { workspace_id, entries }`. Events: `ProjectSharesChanged
{ project_id, rules }`, `SharesApplied { workspace_id, actions, error }`.
`Response::Snapshot` gains `worktree_shares: Vec<ShareRule>` (all projects; the
rows are tiny and the settings section needs them without a round trip — the
`agent_profiles` precedent, `response.rs:112`), and `client::Store` caches them
the way it caches profiles (`store.rs:340`).

Everything new is `#[non_exhaustive]` with `serde(default)` on added struct
fields, so an older client stays decodable.

**Acceptance:** `cargo test -p protocol` round-trips every new message;
`cargo run -p protocol --bin export-fixtures && pnpm codegen` regenerates
`apps/tauri/src/runtime/generated/fixtures.ts` with no manual edits.

### F2 — Persistence

**Files:** `crates/persistence/src/migrations.rs` (append **migration 8**, never
edit an existing one), `repositories/shares.rs` (new), `repositories/mod.rs`,
`persistence/src/tests.rs`.

```sql
CREATE TABLE worktree_shares (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    strategy   TEXT NOT NULL,   -- JSON: {"kind":"clone"} | {"kind":"run","command":"pnpm i","timeout_secs":600}
    enabled    INTEGER NOT NULL,
    position   INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_worktree_shares_project ON worktree_shares (project_id);
CREATE UNIQUE INDEX idx_worktree_shares_path ON worktree_shares (project_id, path);
```

`ON DELETE CASCADE` because migration 2 (`REFERENTIAL_ACTIONS`) exists precisely
to stop new tables from repeating the bare-`REFERENCES` mistake: removing a
project must not leave orphan rules. `strategy` is JSON for the same reason
`agent_profiles.args_json` is: nothing ever queries inside it.

`ShareRepo`: `list_for_project`, `list_all`, `replace_for_project(project_id,
&[ShareRule])` in one transaction (the GUI edits a set, not a row), `delete`.

**Acceptance:** a migration test that opens a v7 database and reads it at v8;
round-trip of every `ShareStrategy` variant through JSON; unique-index conflict
surfaced as a typed error the daemon maps to `ErrorCode::Conflict`.

### F3 — The provisioning engine

**Files:** `crates/daemon/src/shares/mod.rs`, `plan.rs`, `apply.rs`, `detect.rs`
(new); `crates/git-service/src/repository.rs` (two additions).

**Path validation (`apply.rs::validate`) — refuse, do not sanitise:**

- relative only, no `..` component, no absolute prefix (today's rule,
  `core.rs:4078`);
- normalised path must stay inside the workspace after `Path::components`
  folding, and the *resolved parent* must not escape it through a symlink;
- never `.git`, never anything under `$GIT_COMMON_DIR`;
- no symlink in the source is followed when copying (`symlink_metadata`, like
  `core.rs:4092`);
- length and component-count caps, so a rule cannot be a path bomb.

**Mechanisms:**

| Strategy | Implementation | Notes |
|---|---|---|
| `Copy` | file: `std::fs::copy` + mode preserved (today's code, kept). Directory: recursive walk, **budgeted** by entries and bytes, requires the D5 confirmation flag on the rule | budget exceeded → `Skipped { reason }`, never a partial tree left behind |
| `Clone` | macOS: `clonefile(2)` via `nix::libc` (`nix` re-exports `libc`, and it is already a daemon dependency: `crates/daemon/Cargo.toml:37`). Linux: `FICLONE` ioctl. Both fall back to `Copy` and set `ShareAction::fallback` | the fallback is **reported**, because a silent 900 MB copy is a different product |
| `Link` | `symlink` into `$GIT_COMMON_DIR/forge/shared/<path>`; store entry created by `AdoptIntoShareStore`, never implicitly | a pre-existing real file at the target is backed up, never overwritten |
| `Run` | `sh -c` from the workspace root, `.process_group(0)`, killed with a **negative** pgid, `wait_with_output()` draining both pipes, reaped — the `run_setup_script` code (`core.rs:4120`) generalised per rule | output to the daemon log; failure is a notice, never a refusal |

**Planning (`plan.rs`) is pure:** `(rules, source_observations, target_observations,
capabilities) -> Vec<ShareAction>`. It decides skip/copy/clone/link/run and the
`fallback` flag from an injected `Capabilities { supports_clone: bool,
same_filesystem: bool }`, so the whole decision table is unit-testable without a
filesystem. Effects live in `apply.rs`; orchestration in `core.rs`. This is the
`idle.rs` split, deliberately.

**Detection (`detect.rs`):** `git status --porcelain --ignored=matching -z` via
`run_git` at the repository root, plus a **depth-limited** root scan; classify by
name (`.env*`, `*.pem`, `*.key` → `Secret`; `node_modules`, `.venv`, `vendor`,
`Pods` → `Dependencies`; `target`, `dist`, `.next`, `build` → `BuildOutput`;
`.turbo`, `.cache` → `Cache`; `.vscode`, `.idea`, `.claude` → `EditorState`).
Sizes come from a **bounded** walk that reports `None` rather than looking
exhaustive — the AGENTS.md rule about clamping before the work, not after.

**git-service additions:** `common_dir(repo) -> PathBuf` (`rev-parse
--git-common-dir`, absolute) and `set_local_excludes(repo, &[&str])`, which
rewrites a delimited block in `$GIT_COMMON_DIR/info/exclude`:

```
# >>> forge: shared files (managed, do not edit)
/.env
/node_modules/
# <<< forge
```

Idempotent, preserves everything outside the block, and is what keeps every
provisioned worktree from reading as dirty in `RefreshWorkspaceStatus` (D7).

**Acceptance:** unit tests for `plan.rs`'s table (every strategy × every observed
state × clone-capable/not); `apply.rs` tests on a temp tree including a severed
link, a refused `..`, a refused `.git/…`, a budget-exceeded directory copy, and a
`Run` whose child spawns a grandchild that must not survive the timeout.

### F4 — Daemon wiring

**Files:** `crates/daemon/src/core.rs`, `crates/daemon/src/config.rs` (doc only),
`crates/daemon/tests/scenario_shares.rs` (new).

- Dispatch arms next to the existing worktree ones (`core.rs:601`, `:619`,
  `:787`) for the six requests of F1.
- `provision_worktree` → `apply_shares(workspace_id, trigger)`; the old
  function's body becomes the `[worktrees] copy` **fallback layer** used when a
  project has no rules.
- Triggers: `CreateWorktree` (`core.rs:2145`, now asynchronous per §4),
  adoption in `reconcile_project_worktrees` (`core.rs:420`, for rows added with
  `managed_by_app = false`), and `ApplyShares`.
- `Inner::provisioning: HashSet<WorkspaceId>` + `Drop` guard, per §4.
- Notices keep today's wording rules and **never contain file contents**; a
  secret-classified path is logged by path only (D6).
- `RemoveWorktree` is unchanged: links are inside the worktree, the store is not.

**Acceptance:** `scenario_shares.rs` drives the protocol end to end against a
real temp repository — create with rules and assert the file lands *after*
`SharesApplied`; adopt an externally created worktree and assert the same; apply
twice and assert the second run is a no-op; sever a link and assert
`GetShareStatus` reports `Severed`; a failing `Run` yields a notice and a usable
worktree.

### F5 — Tauri host plumbing

**Files:** `apps/tauri/src-tauri/src/runtime/workbench.rs`, `commands.rs`,
`runtime/commands.rs`, `runtime/bridge.rs`.

The reads and the apply go on the **workbench worker**, not on
`RuntimeCommand`: that worker exists because "a `git diff` of a large checkout is
seconds of subprocess" (`commands.rs:26`) and provisioning is worse. New
`WorkbenchCommand` variants and their emissions, following `ListBranches`
(`workbench.rs:327`):

| Command | Emits |
|---|---|
| `DetectShareCandidates { project }` | `workbench:share_candidates` / `…_failed` |
| `PreviewShares { workspace }` | `workbench:share_plan` / `…_failed` |
| `GetShareStatus { workspace }` | `workbench:share_status` / `…_failed` |
| `ApplyShares { workspace, only }` | ack only; outcome arrives as the `SharesApplied` daemon event |

`SetProjectShares` and `RemoveShareRule` are small, fast writes and go on
`RuntimeCommand` next to `SaveAgentProfile` (`runtime/commands.rs:280`,
`bridge.rs:1081`).

*Add a file…* needs a **`pick_file` host command**: `commands.rs:41` only has
`pick_directory` (`.pick_folder`). The sibling is the same shape with
`.pick_file` and a starting directory — the picker is modal, so like
`pick_directory` it answers on its own channel and the caller turns the path
into an ordinary command rather than awaiting it inside an event handler.

**Acceptance:** `pnpm codegen` regenerates fixtures; no new dependency on
`protocol` from the host (it reaches wire types through `client`'s re-exports).

### F6 — GUI

**Files:** `apps/tauri/src/settings/SharedFiles.tsx` (new),
`settings/SettingsRoute.tsx` (section registration),
`apps/tauri/src/runtime/api.ts`, `runtime/events.ts`, `store/forgeStore.ts`,
the New-workspace dialog, the workspace row, `styles/settings.css`.

```
┌ Shared files ─────────────────────────────── project: forge-node ─┐
│ Rules apply to every workspace of this project.                   │
│ Using: project rules  (global [worktrees] list ignored) [Import…] │
│                                                                   │
│  path                strategy        state in 3 worktrees         │
│  .env                Link      ▾     ●●●  applied            ⋮    │
│  .env.local          Copy      ▾     ●●○  1 missing   [Sync]  ⋮    │
│  node_modules/       Clone     ▾     ●●●  applied · 1 fell back ⋮  │
│  .venv/              Run: uv sync ▾  ○○○  not applied [Sync]  ⋮    │
│  config/local.pem    Copy      ▾     ●●●  applied            ⋮    │
│                                                                   │
│  [+ Add a file…]  [+ Add a folder…]  or type a path:  [________]  │
│                                                                   │
│  Detected, not shared yet                        [Scan again]     │
│  target/       build output   4.2 GB   [Add ▾]                    │
│  .claude/settings.local.json  editor   1 KB   [Add ▾]             │
│                                                                   │
│  [Preview changes]                        [Apply to all worktrees]│
└───────────────────────────────────────────────────────────────────┘
```

Rules of the surface:

- **Adding is never limited to what was detected** (§2.4): the two picker
  buttons and the path field sit above the detected list, not inside it, so a
  brand-new file nobody has scanned for is a first-class row. The path field
  validates as it is typed and names the reason it refuses.
- The row menu (`⋮`) is **Re-apply**, **Disable**, **Move up/down** and
  **Remove…**. Remove opens the `ShareCleanup` choice (§2.4) with *Leave* as the
  default, states in words what each option touches ("delete the copy in 3
  worktrees", "turn 3 links into real files"), and says which workspaces it will
  skip because their file was modified.
- Editing a strategy marks the row *pending* rather than rewriting anything: the
  change reaches a workspace on the next apply, from the row's Re-apply or from
  the section's Apply.
- The strategy picker explains itself in one line per option, in the terms of
  §5 of the alternatives doc ("Link: one file, every worktree — an edit is
  visible everywhere. Some editors replace links when they save; Forge detects
  it and tells you.").
- Choosing `Link` for a path that exists as a real file opens the **adopt
  confirmation**: what moves, where the backup goes, how to undo it. Nothing
  moves without it.
- `Copy`/`Clone` on a directory above the size threshold asks once and shows the
  number (D5). `Clone` on a filesystem without CoW support is labelled *"will
  copy (this volume has no copy-on-write)"* **before** it runs.
- A secret-classified path shows a one-line warning when the chosen strategy
  duplicates it at rest (D6). Contents are never rendered.
- Per-workspace state comes from `GetShareStatus`; `Severed` gets a "Re-link"
  action, `Diverged` a "Re-copy".
- The New-workspace dialog shows a **read-only summary** ("2 files linked,
  `node_modules` cloned, `uv sync` will run") with a link into this section, and
  the workspace row shows a *setting up…* state while `SharesApplied` is
  pending (§4).

**Acceptance:** a path that is not in the detected list can be added, edited and
removed end to end, and survives a restart; a tracked path and a path outside the
repository are both refused with their reason; removing a rule with *Leave*
changes nothing on disk while *RemoveInjected* removes only what Forge wrote; no
hardcoded colors (theme tokens only); the list virtualises past a threshold, or
is capped with a "showing N of M" line (C7 — a candidate list on a monorepo can
be long); the section renders correctly with zero rules, with rules and no
worktrees, and while the daemon is disconnected.

### F7 — Docs and back-compat

- `docs/worktrees.md` §*Provisioning* rewritten around rules, the store, the
  triggers and the asynchronous behaviour.
- `docs/config.example.toml`: `[worktrees] copy` / `setup_script` documented as
  the **global default layer**, with a pointer to the per-project section.
- `docs/protocol.md`: the six requests, two responses, two events.
- `docs/persistence.md`: migration 8.
- `AGENTS.md`: the provisioning bullet updated (it currently states the inline,
  best-effort behaviour), plus the new invariant: *provisioning never runs on the
  request thread and never holds the core lock across filesystem work*.
- `harness/CHECKPOINTS.md`: C5 gains the coalescing-flag-unwound-by-`Drop` check
  for `provisioning`, mirroring `fetching`/`pr_opening`.

## 6. Failure taxonomy and what the user sees

| Situation | Behaviour |
|---|---|
| Source path missing | `Skipped { reason: "not present in the source workspace" }`. A rule is a wish, not a requirement (today's rule, kept). |
| Target already a regular file with different content | Backed up to `forge/backups/<ts>/`, then applied. Never silently overwritten. |
| `Clone` on a non-CoW volume | Applied as a copy, `fallback: true`, one notice per apply (not per file). |
| Directory over budget | `Skipped`, with the number, and a "copy anyway" action. Nothing partial is left. |
| `Run` exits non-zero / times out | Notice with the exit status and the last log lines; worktree stays usable. Process group killed with a negative pgid. |
| Link severed by an editor | `GetShareStatus` → `Severed`; the row offers "Re-link" (backs the new file up first). |
| `$GIT_COMMON_DIR` unreachable (not a git project) | `link` rules are `Skipped { reason }`; `copy`/`clone`/`run` still work. |
| Two applies race on one workspace | Second one coalesces onto the first (§4), it does not queue. |
| A removed rule's file was edited in a worktree | `RemoveInjected` skips it and names the workspace: a modified file is somebody's work, not our leftover. |
| A rule is added for a path that does not exist yet | Stored and reported `Skipped`; it starts applying the day the file appears. |
| A rule is added for a tracked path | Refused at the moment of adding, with the reason (§2.4). |

## 7. Tests

- **Unit, pure:** `plan.rs` decision table; path validation refusals; the
  `info/exclude` block rewriter (idempotence, foreign content preserved);
  classification of ~30 real-world paths.
- **Unit, filesystem:** each mechanism on a temp tree, including permissions
  preservation, symlink-in-source refusal, severed-link detection, budget stop.
- **git integration** (`crates/git-service/tests/`): `common_dir` from the main
  checkout *and* from a linked worktree returns the same absolute path; excludes
  block written into the common dir is visible from every worktree. These follow
  the existing rule — real temp repositories, and they must **fail**, not skip,
  in CI where git exists.
- **Daemon scenario** (`crates/daemon/tests/scenario_shares.rs`): the five
  end-to-end cases of F4, plus one that creates a worktree in a terminal
  (`git worktree add` directly, as `test-support` is allowed to) and asserts
  `RefreshProject` both adopts **and** provisions it.
- **Cost:** a test asserting `ApplyShares` returns its ack before the work
  finishes (the §4 invariant), and that no `SharesApplied` handler holds the core
  lock across I/O (reviewed by C5/C7, asserted by a timing bound).
- **Rule editing** (`scenario_shares.rs`): add a rule for a path the scan never
  reported and assert it applies; change its strategy and assert nothing moves
  until the next apply; remove it with each `ShareCleanup` and assert *Leave*
  touches nothing, *RemoveInjected* removes only untouched injections and skips a
  modified one, and *Materialize* leaves a real file in every workspace and the
  store entry intact.
- **Front-end:** `SharedFiles` rendering states; `pnpm test` for the classifier
  mirror if any lives in TS (preferably none — the taxonomy stays in Rust).

## 8. Sequencing

```
F1 domain+protocol+client ──┬─► F2 persistence ──► F4 daemon wiring ──► F7 docs
                            └─► F3 engine ───────┘
F1 ──► F5 host plumbing ──► F6 GUI
```

F2 and F3 are independent of each other and can run in parallel after F1. F6 is
the only block that needs both sides finished to be demonstrable; it can be built
against `fixtures.ts` before F4 lands.

## 9. Explicitly not in v1

- The `env` strategy (`CARGO_TARGET_DIR`, `PNPM_STORE_DIR`, …). Designed for —
  `ShareStrategy` is `#[non_exhaustive]` — not built.
- The `post-checkout` git hook (D4). It writes into the user's repository and
  runs when Forge is not there to report failures; it earns its own feature once
  the rest is proven.
- A committed `.forge/worktrees.toml` (C3). The precedence chain is designed so
  it can be imported later without moving the source of truth.
- Watcher-driven sync (M9), bind mounts (M8), hardlinks (M4). Rejected with
  reasons in the alternatives doc.
- Secret-manager integration (`op inject`, `sops`). Reachable today through a
  `Run` rule, which is the honest amount of support to claim.

## 10. Open questions, small enough not to block

1. **Directory copy budget default.** Proposed: 250 MB / 50 000 entries before
   the confirmation. Needs one real measurement on a `node_modules` of this
   project's size.
2. **Do adopted (unmanaged) worktrees get provisioned automatically, or only on
   demand?** Proposed: automatically, since D8 says rules are project-wide, but
   with the first apply of an unmanaged worktree asking once — it is a directory
   Forge did not create.
3. **Store location when the project is not a git repository.** Proposed: no
   `link` strategy at all there (§6), rather than inventing a store outside git.
