# Orca teardown: git worktrees and workspace provisioning

Read of the extracted Orca bundle (MIT, `out/`) against Forge's own worktree
path. Analysis only — no Forge source was changed, and nothing below proposes
copying Orca code. Orca paths are relative to the extracted `out/` root; Forge
paths are `file:line` in this repository.

Scope: how a worktree gets its name and its path, what happens on provision and
when provisioning fails, how worktrees made outside the app are discovered and
classified, and how filesystem work is bounded.

---

## 1. How Orca does it

### 1.1 Three name axes, deliberately separate

Orca never derives one name and reuses it. It carries three, computed by three
different rules:

- **Display name** — `sanitizeWorktreeDisplayName` (`main/index.js:17417`).
  Strips C0/C1 control characters *by code point* (there is a lint rule against
  control-character regexes), strips Unicode bidi overrides `U+202A–U+202E` /
  `U+2066–U+2069`, collapses whitespace, caps at 120 chars. Never touches
  Unicode letters — this is a label, not a path.
- **Filesystem slug** — `sanitizeWorktreeName` (`main/index.js:17408`). Emoji are
  replaced with shortcodes *before* sanitizing, then `[^\p{L}\p{N}._-]+` → `-`,
  dashes collapse, `..` → `.`, leading/trailing `.`/`-` stripped. Empty-after-
  emoji yields `"workspace"`; empty, `.` or `..` **throws**. Note the character
  class keeps Unicode letters, so `café` stays `café` on disk.
- **Branch name** — built independently from a configurable prefix
  (`shared/branch-prefix.js`) joined to a leaf. `normalizeBranchPrefix` strips
  surrounding slashes and collapses internal runs so the `${prefix}/${leaf}`
  join always produces exactly one separator, and `getBranchPrefixIssue`
  re-implements the relevant `check-ref-format` rules (control/space, `~^:?*[\`,
  `..`, `@{`, leading `-`, trailing `.`, per-segment leading `.` / trailing
  `.lock`) so settings can give live feedback. `assertBranchPrefixValid` fails
  fast at create time — git stays the source of truth, this only buys a legible
  error.

`shouldSetDisplayName` (`main/index.js:17509`) stores a display name **only when
it differs** from both the branch and the slug, so the common case stores
nothing and the UI has one string to render, not two identical ones.

### 1.2 Default names come from a pool, and are retired rather than recycled

Orca's generated names are marine creatures
(`shared/worktree-name-suggestion.js`). The interesting decision is not the pool
— it is `shared/worktree/retired-name-registry.js`:

> A name whose workspace was deleted still owns its old directory path in any
> agent CLI that keys conversation state by cwd, so reissuing it hands the next
> occupant someone else's history.

So deleted names are **retired**, never reused. To keep that registry from
growing without bound, names are tiered (`nautilus`, `nautilus-2`, …) and a
completed tier collapses into an integer watermark (`exhaustedTiers`), with the
individual names dropped. `clampExhaustedTiers` caps the watermark at 999 999 —
a value past what `creatureNameTier` can parse would silence lookups forever.
`creatureNameTier` returns `null` for anything the suggester never emits
(`nautilus-2-3`, a non-pool base), so a user-typed `fix-login-2` can never read
as retired because tier 2 happens to be spent.

`shared/worktree/retired-name-cache.js` adds the client half: a failed refresh
holds the *previous* answer rather than blanking, because a refresh fires on
every workspace-list mutation and blanking would suggest a spent name in exactly
the window where the create form is asking for one.

### 1.3 Path: layout, then a containment re-check

`computeWorktreePath` (`main/index.js:17431`):

```
workspaceRoot = repo.worktreeBasePath?.trim() || settings.workspaceDir
                (relative values resolved against repo.path)
path          = nestWorkspaces ? workspaceRoot/<repoName>/<slug>
                               : workspaceRoot/<slug>
```

`getRuntimePathOps` picks `path.win32` or `path.posix` from the *shape of the
input*, not from `process.platform`, because a repo can be a WSL UNC path or an
SSH host path on a Windows client.

The slug sanitizer is not trusted to guarantee containment. After the join,
`ensurePathWithinWorkspace` (`main/index.js:17423`) re-derives
`path.relative(workspaceRoot, target)` and throws `"Invalid worktree path"` if
it is absolute, `..`, or starts with `../`. Two independent guards for one
property.

### 1.4 The create loop: name, branch and directory are one collision test

`main/index.js:121283–121347`. Up to **100 attempts**, and each attempt must
clear every axis at once:

1. Candidate slug from the suffix (`getWorktreeCreateCandidate`, or the tier-
   aware `getGeneratedWorktreeCreateRetryCandidate` for pool names).
2. Skip immediately if the name is retired — the skip does **not** consume an
   attempt.
3. Resolve the branch name for that candidate; classify a conflict as `local`
   or `remote` (`getBranchConflictKind`).
4. If the branch already carries a PR that is not the one the user selected,
   keep going (`getLocalGitHubPrForBranch`).
5. `existsSync(worktreePath)` — the **on-disk directory is part of the
   collision test**, not just the app's own model.

Exhaustion produces three distinct errors naming which axis lost: the PR, the
branch (local vs remote), or the name.

`shared/new-workspace/worktree-create-retry-policy.js` mirrors a *client-side*
retry policy on top of this (25 attempts, a pattern list of retryable conflict
messages) for mixed-version runtimes whose host does not do its own suffix loop,
plus a shared `WORKTREE_CREATE_DEDUPE_TTL_MS = 60_000` so a replayed idempotent
create lands inside the host's dedupe window instead of building a second
worktree.

### 1.5 Base ref is fully qualified before `worktree add`

`shared/worktree/base-ref.js`, in full:

> `git worktree add` receives a revision, so short names can collide with tags.
> Prefer the namespace implied by Orca's base picker: remote display names like
> `origin/main` first, otherwise local branches.

A base containing `/` is probed as `refs/remotes/<base>` then `refs/heads/<base>`;
a bare name only as `refs/heads/<base>`; anything already starting with `refs/`
passes through. The resolved ref is what reaches git, and it is persisted as the
worktree's creation base.

### 1.6 `worktree add` and its timeout

`performAddWorktree` (`main/index.js:68722`):

- new branch: `worktree add [--no-checkout] --no-track -b <branch> <path> <base>`
- existing branch: `worktree add [--no-checkout] <path> <branch>`

Then, best effort with a `console.warn` on failure: read `push.autoSetupRemote`
and set it `--local` if unset. `--no-track` plus `push.autoSetupRemote` is a
deliberate pair — upstream configuration is deferred from create to first push.

Timeout: `resolveWorktreeAddTimeoutMs` (`main/index.js:68156`) floors at 180 s,
ceilings at 30 min, and is overridable by `ORCA_WORKTREE_ADD_TIMEOUT_MS` —
clamped **in both directions**, with a warning naming the clamp when the request
was out of range or unparseable. `worktree list` gets 30 s; removal preflight and
registration 30 s each.

The sparse variant (`addSparseWorktree`, `main/index.js:68767`) is the one path
that **unwinds**: `--no-checkout`, `sparse-checkout init --cone`,
`sparse-checkout set`, `checkout`; any failure clears the persisted creation base
and removes the worktree it just created.

### 1.7 Provisioning: three modes, one engine, one budget

Everything Orca puts into a fresh worktree goes through
`materializeWorktreePaths` (`main/index.js:119728`) in one of three modes:

| Mode | Source of the list | Rule |
|---|---|---|
| `link` | `repo.symlinkPaths` | symlink (junction/dir on Windows) |
| `share` | hooks `worktree.sharedDirectories` | **directories only**, symlink |
| `copy` | `.worktreeinclude` in the repo root | files or directories, real copy |

Two rules bind all three:

- **Only gitignored paths are eligible.** Both `resolveWorktreeIncludePaths`
  (`main/index.js:119903`) and `resolveWorktreeSharedDirectories`
  (`main/index.js:119953`) end with `checkIgnoredPaths(...)` and keep only the
  entries git ignores, warning about the rest. Tracked files arrive with the
  checkout; copying one over it would shadow git. `checkIgnoredPaths` batches
  through a single `check-ignore -z --stdin` per ≤1 MiB stdin chunk, and treats
  exit **1** as the success case.
- **Path safety, then existence, then classification.** `getSafeRelativePath`
  rejects absolute / `..` / `.git`-rooted entries; an existing target is skipped
  rather than overwritten; `lstat` + `stat` decide file vs directory vs symlink
  before anything is created.

`.worktreeinclude` is itself bounded before it is parsed: ≤256 KiB file, ≤1000
entries, no globs, no negations, comments and duplicates dropped
(`main/index.js:119869`).

**The copy budget is charged before the copy.** `createWorktreeCopyBudgetTracker`
(`main/index.js:119618`) holds 2 GiB / 50 000 entries and a separate *sizing*
allowance of `5 × maxEntries` walked inodes. `measureCopySize` walks the source
first and returns a verdict — `bytes`, `entries` or `sizing` — and only a
`withinBudget` verdict debits the tracker and proceeds. On macOS it tries an APFS
clone (`/bin/cp -n -c -R`) first, and when the clone is unavailable and the real
copy would exceed the remaining byte budget it throws
`WorktreeCopyBudgetFallbackError` rather than falling back into a copy it already
knows it cannot afford.

Skips are not silent. `formatWorktreeIncludeCopyWarning`
(`main/index.js:119649`) folds them into one sentence naming ≤5 entries plus a
count, distinguishing "over budget" from "we ran out of budget to *measure* what
to copy", and separately flagging entries that may hold a **partial** copy from
an interrupted directory clone.

### 1.8 What happens when provisioning fails

Only `git worktree add` is fatal. Everything after it is best effort, and the
failure modes are separated rather than collapsed:

- symlink / shared-dir failures: `console.error`, continue.
- `.worktreeinclude` skips: returned as a `warning` string on the create
  response, so it reaches the user, not just the log.
- setup-hook resolution, default-tab resolution and setup-runner construction
  are each wrapped in their own `try` (`main/index.js:121459–121483`), so a
  broken hooks file costs the tabs, not the workspace.
- The setup script itself is not run inline — it becomes a *terminal* the user
  watches (`createSetupRunnerScript`, `spawnLocalStartupAndSetupTerminals`).

Progress is reported while this happens: `emitCreateWorktreeProgress(…,
"fetching" | "creating", creationId)`, and the whole create is instrumented with
a phase recorder (`git_worktree_add`, `resolve_worktreeinclude`,
`copy_worktreeinclude`, `prepare_setup`, `spawn_startup_terminal`, …) returned to
the caller as `timing`.

### 1.9 Detecting worktrees made outside the app

**Not a filesystem scan.** Discovery is `git worktree list --porcelain` (with a
`-z` capability probe: `shared/git-worktree-command-capabilities.js` treats exit
**129** as the locale-independent "flag unsupported" signal for old native/WSL/SSH
git). Each row is then *classified*, `shared/worktree/ownership.js:195`:

1. **`orca-managed`** — the stored metadata carries a strong marker
   (`orcaCreatedAt`, `orcaCreationWorkspaceLayout`, `pushTarget`, `sparseBaseRef`,
   …). Only metadata proves Orca created it; a plain `git worktree add` can
   target Orca's own workspace folder.
2. **`agent-scratch`** — the path sits under a known agent scratch root
   (`.claude/worktrees`, `.gsd-workspaces`) *anchored to a registered checkout
   path* (`shared/worktree/visibility-sources.js:688`). Checked **before** the
   layout heuristics, with a bug reference (#9388): a looser match hid legitimate
   user worktrees. A companion rule treats whole repos minted under
   `.codex-tmp`, `.codex/vendor_imports`, `.claude/skills` as agent-internal.
3. **`external`** — inside a known nested Orca layout but without metadata.
4. **`unknown-legacy`** — everything else, including anything under a *flat*
   layout, where Orca cannot tell its own directories from a sibling's.

Visibility is then a separate five-input decision
(`shared/worktree-visibility-resolution.js`): selected checkout → always;
Orca-managed → always; explicitly imported path → always; matched visibility
source → that source's preference; agent-scratch → default hidden; legacy repo →
shown. `applyMetadataFallbackVisibility` **fails open** when metadata is
unreadable, except for scratch. A repo added before a hardcoded rollout date
(`Date.UTC(2026, 4, 23)`) is grandfathered to the old default, and a one-shot
migration rewrites every affected repo row.

New hidden worktrees accumulate into an "inbox"
(`shared/external-worktree-inbox.js`) diffed against a stored baseline path set,
offered once the user has dismissed the initial prompt and not suppressed
discovery.

**Read dedupe.** `listWorktrees` (`main/index.js:68598`) shares one in-flight
promise keyed by `(repoPath, wslDistro, timeout, generation)`. A create or remove
calls `bumpWorktreeScanGeneration`, which increments **only if a scan is
currently in flight** — a mutation invalidates the shared read instead of letting
a caller receive a listing taken before it.

### 1.10 Bounding a filesystem scan

The only place Orca genuinely walks a tree for a workspace is the disk-usage
view, and it is the cleanest budget code in the bundle
(`shared/workspace-space-scan-budget.js`):

- Hard caps: 100 000 entries, 64 MiB of *retained* scan state. `clampLimit`
  rejects anything that is not a positive safe integer and takes `min(requested,
  maximum)` — a caller can only ask for **less**.
- `retainWorkspaceSpaceScanEntry` charges `name.length * 2 + 512` bytes and
  throws `WorkspaceSpaceScanCapacityError` **before** the entry is pushed.
- A listing's shared parent-path string is charged **once**, with its first
  entry — charging it per entry scaled the estimate by checkout depth rather
  than by live heap.
- `releaseWorkspaceSpaceScanEntries` gives the charge back when a directory frame
  is retired, so the cap tracks *live* retention rather than accumulating across
  the whole traversal. A rejected or cancelled listing returns its charge in a
  `catch` before rethrowing.
- The traversal (`shared/workspace-space-entry-traversal.js`) is a fixed worker
  pool over an explicit frame stack — no promise or closure per entry. Idle
  workers **park on a promise woken by `wakeWorkers()`**; there is no poll loop.
- Output is compacted too: `compactWorkspaceSpaceItems` caps at 48 top-level rows
  and folds the rest into an `Other` bucket carrying the omitted count and bytes.

### 1.11 Removal

`performRemoveWorktree` (`main/index.js:68819`) refuses a git-locked worktree up
front with a message naming the lock reason and the exact `git worktree unlock`
recovery. The fast path renames the directory into a trash location **first**,
then deregisters it with git, restoring from trash if deregistration fails, and
schedules the actual deletion. `git worktree remove` categorically refuses a
worktree containing an initialised submodule even when everything is clean, so
that specific refusal is detected by message
(`shared/worktree/submodule-removal.js`), cleanliness is re-proved, and the call
retried with `--force`.

Branch deletion after removal tries `-d`, prunes and retries on
"checked out in another worktree", and on a not-fully-merged refusal *preserves*
the branch and reports it. `forceDeleteLocalBranch` uses
`update-ref -d refs/heads/<b> <expected-head>` — a compare-and-delete, so a branch
that moved after the workspace was deleted is never dropped.

`shared/worktree/removal.js` is a classifier shared between main and renderer so
the force-delete button and the error text can never disagree — including a
distinction between "we watched this PTY stay alive" and "we could not confirm",
because waiving those is a different user decision.

---

## 2. Side by side

| Orca mechanism | Forge equivalent | Verdict |
|---|---|---|
| Display name / slug / branch as three sanitizers (`main/index.js:17408`, `:17417`, `shared/branch-prefix.js`) | `slugify` `crates/git-service/src/worktree.rs:44`; `Workspace::display_name` `crates/domain/src/workspace.rs:71`; `inferred_display_name` `crates/daemon/src/core.rs:4609` | **Forge equivalent.** Forge's ASCII-only slug is stricter and its `..`/`.`/empty → `worktree` rewrite (`worktree.rs:68`) buys containment in a pure, testable function rather than needing a post-join filesystem re-check. No branch prefix — see rec. 7. |
| Containment re-check after the join (`ensurePathWithinWorkspace`) | `worktrees_root.join(project_id).join(&slug)` `core.rs:2190` | **Different tradeoff, Forge is fine.** Forge's slug cannot contain `/` at all, so the join is safe by construction. Orca needs the second guard because its slug keeps Unicode and multi-segment inputs. |
| Base ref fully qualified before `worktree add` (`shared/worktree/base-ref.js`) | `base` passed raw to `worktree add … <base>` `worktree.rs:192` | **Gap.** See rec. 1. |
| 100-attempt loop testing name + branch + PR + `existsSync(path)` (`main/index.js:121283`) | `unique_slug(&existing_slugs, …)` over in-memory `managed_by_app` workspaces `core.rs:2176`, `worktree.rs:84` | **Gap.** The filesystem is never consulted. See rec. 2. |
| `--` never used; `--no-track -b <b> <path> <base>` (`main/index.js:68733`) | `reject_option_like` + `--` at both positional sites `worktree.rs:117,189,193` | **Forge is better.** Orca hardens the branch *prefix* against `check-ref-format` but still hands git bare positionals. |
| Add timeout 180 s, env-overridable, clamped both ways (`main/index.js:68156`) | ADR-008 fixed 30 s `command.rs:43` | **Different tradeoff.** Forge's single budget is an ADR, and `worktree add` is local. Orca needs 180 s because it also runs over SSH/WSL. Leave it. |
| `push.autoSetupRemote` set per worktree | Not set; upstream comes from `branch.autoSetupMerge` (tested, `docs/worktrees.md`) | **Different tradeoff, Forge is fine.** Forge pushes only from `CreatePullRequest`. |
| Copy list = gitignored entries only, via one batched `check-ignore` | `config.worktrees.copy` copied unconditionally `core.rs:4464` | **Gap.** See rec. 4. |
| Copy budget measured before copying: 2 GiB / 50 k entries / 5× sizing walk (`main/index.js:119618`) | Plain-file-only refusal, no size cap `core.rs:4483` | **Partial gap.** Forge's shape guard is cheaper and blocks the `node_modules` case outright, but there is no byte clamp. See rec. 3. |
| Skips reported as a structured `warning` on the create response | `self.notice(NoticeLevel::Warning, …)` `core.rs:2249` | **Forge equivalent.** Same reachability, one channel. |
| Provisioning best effort; only `worktree add` fatal | Same, documented at `core.rs:2198` and AGENTS.md "Boundaries And Invariants" | **Forge equivalent** — and Forge's is the clearer statement of the rule. |
| Setup script runs as a *watched terminal*, create returns first | `run_setup_script` inline before the ack `core.rs:2201,2260` | **Gap (behavioural).** See rec. 5. |
| Progress events during create (`fetching` / `creating`) + phase timings | None; single `Ack` after everything | **Gap**, folded into rec. 5. |
| Ownership classification: managed / agent-scratch / external / legacy (`shared/worktree/ownership.js:195`) | `managed_by_app: bool` `workspace.rs:73`; everything listed is adopted `core.rs:504` | **Gap.** Forge has two states where four are observable, and `.claude/worktrees` is exactly the case that hurts here. See rec. 6. |
| Five-input visibility policy + rollout-date grandfathering + import inbox | None | **Different tradeoff — deliberately not Forge's problem.** See §4. |
| Creature-name pool + retirement registry + tier compaction (3 modules) | Names come from the branch the user typed | **Forge is better for its shape.** See §4. |
| `listWorktrees` in-flight dedupe keyed on a generation counter (`main/index.js:68598`) | `list_worktrees` called per rescan / create `repository.rs:205` | **Different tradeoff; do not copy.** A TTL shorter than the gap between two user actions is not a cache (AGENTS.md). Forge's calls are already serialized by the request path. |
| Trash-rename-then-deregister removal with restore-on-failure (`main/index.js:68845`) | DB-first, then `git worktree remove`, self-healing on next rescan `core.rs:2336–2369` | **Different tradeoff, Forge is fine.** Forge already gets the important half (a failure leaves user files intact) without a trash root and a sweeper. |
| Submodule-refusal detection + re-prove-clean + `--force` retry | None | **Small gap**, noted in rec. 8. |
| Locked-worktree refusal with the `git worktree unlock` recovery | `precheck_remove` covers dirty + merge/rebase only `worktree.rs:236` | **Small gap**, noted in rec. 8. |
| Compare-and-delete branch removal (`update-ref -d <ref> <expected>`) | Forge never deletes a branch, by any path | **Forge is better.** The safest version of this decision. |
| Scan budget: clamp-before-allocate, charge-then-push, release-on-retire, parked workers (`shared/workspace-space-scan-budget.js`) | No filesystem walk exists on the worktree path at all | **Forge is better here.** Discovery is `git worktree list --porcelain`, bounded by git and ADR-008. The budget *shape* still applies to `copy_into_worktree` — rec. 3. |
| `-z` capability probe, exit-129 detection | `--porcelain`, no `-z` `repository.rs:206` | **Forge is better for its targets.** Don't add a probe for a git Forge does not support. |
| Worktree lineage graph with cycle detection (`shared/resolved-worktree-lineage.js`) | Parent/child lives on `Session`, not `Workspace` | **Different tradeoff; do not copy.** Two parallel graphs is worse than one. |

---

## 3. Recommendations, ranked

### R1 — Qualify the base ref before `git worktree add` *(highest value, smallest diff)*

**What.** In `crates/git-service/src/worktree.rs:166–195`, `base` reaches git as
a bare revision. Per `gitrevisions`, a short name resolves in the order
`refs/<name>` → **`refs/tags/<name>`** → `refs/heads/<name>` →
`refs/remotes/<name>`. A repository with a release tag named `main`, or a base
like `v2` that exists as both a tag and a branch, silently starts the new branch
from the **tag**. Git prints an ambiguity warning to stderr and proceeds; Forge
captures stderr and discards it on success, so nothing surfaces.

**Change.** Before building the args, resolve `base`:
- pass through unchanged if it starts with `refs/` or is a full hex oid;
- if it contains `/`, probe `refs/remotes/<base>` then `refs/heads/<base>`;
- otherwise probe `refs/heads/<base>` only;
- fall back to the raw value when neither exists (so an explicit tag or SHA
  still works).

Probe with `run_git(Some(repo), &["show-ref", "--verify", "--quiet", ref])` —
one extra local subprocess per create, inside the ADR-008 budget, never
`run_git_network`.

**Files.** `crates/git-service/src/worktree.rs` (the resolver plus `create`);
test in `crates/git-service/tests/integration.rs` alongside
`a_worktree_created_from_a_remote_ref_tracks_it`.

**Could break.** A caller that deliberately passes a tag: covered by the raw
fallback, since the probe only *prefers* the branch namespace when the branch
exists. A caller passing a short SHA — add the hex check, or accept the fallback
path. `apps/tauri already passes `origin/<name>`, which
gains a correct `refs/remotes/` resolution and keeps
`branch.autoSetupMerge` behaviour (the start point is still a remote-tracking
ref).

### R2 — Make the slug collision test include the filesystem

**What.** `core.rs:2176–2189` builds `existing_slugs` from in-memory workspaces
that are `managed_by_app`. `remove_worktree` documents its own residual risk at
`core.rs:2345–2350`: rows deleted, `git worktree remove` then failing, leaving a
directory on disk with no row pointing at it. The next create for the same
branch picks the same slug, and `git worktree add` fails with a bare
`CommandFailed` whose stderr the user cannot act on. The same happens after a
crash between `git_service::create` and the `upsert` at `core.rs:2221`.

**Change.** After `unique_slug`, loop (bounded, 100 like Orca) while
`path.exists()`, appending the next suffix. Also seed `existing_slugs` from
unmanaged workspaces whose parent is `worktrees_root/<project_id>` — cheap, and
covers a worktree Forge adopted at rescan that happens to live in its own root.
The check is `exists()` per candidate, not a directory walk.

**Files.** `crates/daemon/src/core.rs` (`create_managed_worktree`), possibly a
`unique_slug_with` variant in `crates/git-service/src/worktree.rs` taking a
predicate so the loop stays testable without a filesystem.

**Could break.** Nothing user-visible: a create next to a stale sibling now picks
`-2` instead of failing. The one behavioural change is that a directory left by
a *user* at that exact path is no longer collided with, which is the point.

### R3 — Clamp the copy before doing it

**What.** `copy_into_worktree` (`core.rs:4464`) refuses anything that is not a
plain file — a genuinely better guard than Orca's recursive walk for the
`node_modules` case, and it should stay. But there is no size bound:
`copy = ["fixtures/dump.sql"]` at 8 GB blocks provisioning for minutes with no
timeout and can fill the worktrees volume. AGENTS.md names `metadata()` before
`File::open` as one of the four patterns that get clamp-before-allocate right,
and the metadata is **already in hand** at `core.rs:4478` for the file-type
check.

**Change.** Add `[worktrees] max_copy_bytes` (default in the low tens of MiB —
`.env`, a certificate, a settings file are all kilobytes) to
`crates/daemon/src/config.rs:108`. In `copy_into_worktree`, compare
`metadata.len()` against it and return a distinguishable "skipped, too large"
outcome; `provision_worktree` turns that into a notice naming the file, the size
and the limit — the Orca lesson from `formatWorktreeIncludeCopyWarning` is that
a skip the user cannot see is a skip they will debug as a missing file.
Optionally also cap the *total* across the copy list, which is the one-line
version of Orca's budget tracker.

**Files.** `crates/daemon/src/config.rs`, `crates/daemon/src/core.rs`
(`copy_into_worktree`, `provision_worktree`), `docs/config.example.toml`,
`docs/worktrees.md`.

**Could break.** A user deliberately copying a large fixture: hence a
configurable limit and a notice, not a hard refusal. Choosing the default too low
turns a working setup into a warning — pick a number and document it in the
example config.

### R4 — Copy only what git ignores

**What.** Orca's rule (`main/index.js:119903`, `:119953`) is that only gitignored
entries are eligible for copying or sharing, because tracked files arrive with
the checkout. Forge's `copy` list is unfiltered, so `copy = ["src/config.rs"]`
plants a tracked file over the checkout's own version and the brand-new worktree
starts **dirty** with nothing explaining why — the exact failure
`provision_worktree`'s doc comment says provisioning exists to prevent.

**Change.** One batched `git check-ignore -z --stdin` over the configured
relative paths (exit **1** is the success case, as `git-service::diff` already
handles for `--no-index`), and copy a non-ignored path with a notice rather than
refusing it. That keeps existing configs working while making the surprise
visible. One extra local subprocess per create, in `git-service` behind
`run_git`, never in `core.rs` directly.

**Files.** `crates/git-service/src/repository.rs` or a small
`git-service::check_ignored`, called from `core.rs::provision_worktree`.

**Could break.** A config that copies a tracked file on purpose — warn-and-copy
keeps it working. If the list is long the stdin batching matters; Orca's 1 MiB
chunking is the reference, but Forge's `copy` lists are configuration-sized, so
one call is enough.

### R5 — Ack `CreateWorktree` before the setup script runs

**What.** `create_managed_worktree` runs `provision_worktree` — including a
`setup_script` with a 120 s default budget — *before* the `WorkspaceCreated`
broadcast at `core.rs:2225` and before `Ack` at `core.rs:2150`. The checkout
exists and is usable the moment `git_service::create` returns
(`core.rs:2196`), but the GUI's `pending_worktree` cannot open it for up to two
minutes, and the client's `Client::request` blocks for the whole time. The server
runs handlers under `spawn_blocking` (`crates/daemon/src/server.rs:217`), so
other clients are not blocked — but one blocking-pool thread is held for the full
budget, and the user sees nothing.

Orca's shape: emit `creating` progress, return the worktree, and stage the setup
script as a terminal the user can watch (`main/index.js:121471–121489`).

**Change.** Split provisioning at its natural seam. The copy pass is sub-second
and can stay inline. Broadcast `WorkspaceCreated` and `Ack` after the copy pass,
then run the setup script on a worker and report the outcome through
`DaemonNotice`. If the script's output is worth seeing, the honest version is
Orca's: launch it as a session in the new workspace, which Forge can already do.

**Could break.** More than the others — treat it as its own feature, not a
drive-by:
- Any caller that assumes a fully provisioned tree on ack. Today that is the GUI,
  which already waits for the broadcast, and `CreateChildSession`'s
  `NewManagedWorktree` policy — which `docs/worktrees.md` records as unwired.
- A worker that can fail must unwind its own state via a `Drop` impl, per
  AGENTS.md — the same rule `fetching` / `pr_opening` follow.
- A second create for the same project while the first script runs needs a
  decision (coalesce or allow); Forge's existing pattern is coalesce-per-key.

### R6 — Classify agent scratch worktrees instead of adopting them

**What.** `reconcile_project_worktrees` (`core.rs:504–526`) turns **every** listed
worktree into a workspace row. Forge is an agent-orchestration terminal; Claude
Code creates worktrees under `.claude/worktrees/` — this repository's own
`git status` shows `.claude/worktrees/` untracked right now. Every subagent
worktree becomes a sidebar row, and rows created that way are `managed_by_app:
false`, so they persist until their directory disappears. Orca hit this hard
enough to cite a bug number (#9388) and to classify scratch **before** any layout
heuristic, because a looser match hid real user worktrees.

**Change.** A small const list of scratch-root segments (`.claude/worktrees`,
`.gsd-workspaces`) matched against `project.root_path`, anchored the way Orca
anchors it — the segments must sit *directly under a registered checkout*, not
appear anywhere in the path — plus a config escape hatch for extra roots. Use it
to *filter* the row (or flag it for a hidden-by-default section), never to delete
anything from disk. Like `WorkspaceStatus`, this is derivable from the path at
rescan time: recompute it, do not add a column.

**Files.** `crates/daemon/src/core.rs` (`reconcile_project_worktrees`), a
predicate in `crates/domain` or `crates/git-service`,
`crates/daemon/src/config.rs` for the escape hatch, `docs/worktrees.md`.

**Could break.** A user who deliberately works inside `.claude/worktrees` loses
the row unless the filter is a view concern rather than a model one — prefer a
flag on the workspace plus a UI filter over dropping the workspace. Also note
that existing rows for such worktrees are already persisted; the change needs to
either reclassify them at rescan or leave them, and leaving them is the safer
default.

### R7 — Optional: a configurable branch prefix

**What.** Orca prepends `<git-username>/` or a custom segment
(`shared/branch-prefix.js`), so worktree branches are attributable on a shared
remote. Forge creates the branch name verbatim from what the user typed
(`branch_picker.rs:172–190`). Low value on a solo checkout, real value on a team
remote.

**Change.** `[worktrees] branch_prefix` in config; join with exactly one `/`
after stripping surrounding and collapsing internal slashes (Orca's
`normalizeBranchPrefix` is the right shape); validate once at config load rather
than per create, since `validate_branch_name` (`worktree.rs:136`) already catches
the result. `plausible_branch_name` (`branch_picker.rs:280`) should preview the
prefixed name so the picker does not show a name different from the branch that
gets made.

**Could break.** The picker's "already checked out" matching and `app_shell`'s
`pending_worktree` both key on the branch name; both would need the prefixed
name, or a mismatch means the GUI never opens the workspace it just created.
That coupling is why this is ranked last despite being a small diff.

### R8 — Optional: two removal refusals worth naming

`precheck_remove` (`worktree.rs:236`) covers dirty and merge/rebase. Two Orca
cases it does not:

- **Locked worktree.** `git worktree remove` refuses one, and Forge surfaces the
  raw stderr. Orca's message names the lock reason and the exact
  `git worktree unlock <path>` recovery.
- **Submodules.** Git categorically refuses to remove a worktree containing an
  initialised submodule even when parent and submodule are clean
  (`validate_no_submodules`, git ≥ 2.17). Orca detects that message, re-proves
  cleanliness, and retries with `--force`
  (`shared/worktree/submodule-removal.js`). Forge would need `LC_ALL=C` for a
  stable match — which `run_git` already pins (`command.rs:218`).

Both are small and both are message-matching, which is fragile. Worth doing only
when someone actually hits them.

---

## 4. What NOT to copy

**The creature-name pool, the retirement registry, and tier compaction**
(`shared/marine-creatures.js`, `shared/worktree/retired-name-registry.js`,
`retired-name-cache.js`, `worktree-name-suggestion.js`,
`new-workspace/worktree-create-retry-policy.js`). Roughly 300 lines, a persisted
per-repo registry, a compaction watermark, and a stale-while-revalidate client
cache — all to solve one problem: agent CLIs key conversation state by cwd, so
reusing a directory name hands the next occupant someone else's history. Forge's
worktree path is `<root>/<project-id>/<slug>` and Forge does not generate names
at all; the name comes from the branch the user typed. The problem does not
exist here. **Keep the observation, not the code**: if Forge ever recycles a
worktree path, agent session state follows the path.

**The visibility policy stack** (`external-worktree-visibility.js`,
`external-worktree-inbox.js`, `worktree/visibility-sources.js`,
`visibility-source-preferences.js`, `worktree-visibility-resolution.js`). Five
inputs, per-repo *and* per-source *and* default preferences, a hardcoded rollout
date (`Date.UTC(2026, 4, 23)`), a migration that rewrites every repo row, and a
discovery inbox with its own baseline set. This is the price of changing a
default on an installed base. Forge has no installed base and can pick one honest
default. Take only the *classification* (rec. 6), not the policy machinery — and
if a preference is ever needed, one boolean, not a lattice.

**Trash-rename-then-deregister removal** (`main/index.js:68845`). Renaming into a
trash directory before deregistering makes removal feel instant and rollback-able
— and it costs a trash root, a scheduled deletion sweeper, and a restore path
that must itself not fail. Forge's DB-first ordering (`core.rs:2336–2351`)
already secures the property that matters: a failure leaves the user's files
intact, and §15.3 step 4 re-adopts the directory on the next rescan. The comment
at `core.rs:2346` reasons this through better than Orca's code does.

**`--no-track` plus a per-worktree `push.autoSetupRemote` write.** Orca defers
upstream configuration from create to push. Forge deliberately relies on
`branch.autoSetupMerge` when the base is a remote-tracking ref — documented in
`docs/worktrees.md`, verified in
`git-service/tests/integration.rs::a_worktree_created_from_a_remote_ref_tracks_it`
— and only pushes from `CreatePullRequest`. Adding a `git config --local` write
to every create buys nothing and adds a failure mode.

**`listWorktrees` in-flight dedupe** (`main/index.js:68598`). Elegant, and the
generation counter that only bumps when a scan is in flight is a nice touch. But
AGENTS.md is explicit that a TTL shorter than the gap between two user actions is
not a cache, and Forge's `list_worktrees` calls are already serialized by the
request path and bounded by ADR-008. Adding a shared-promise layer would add a
generation counter, an in-flight map, and a new place for a stale read to hide,
for no measured win.

**The workspace-space traversal engine**
(`shared/workspace-space-entry-traversal.js`, ~220 lines). It is good code — a
fixed worker pool over an explicit frame stack, workers parked on a promise woken
by `wakeWorkers()` rather than polling, so it does **not** violate Forge's
sleep-and-check rule. But it exists for a disk-usage feature Forge does not have,
and a Rust port would want a work-stealing deque or `rayon`, not a transliteration
of a JS event-loop design. Steal the *budget shape* (rec. 3), not the walker.

**The `-z` capability probe and exit-129 detection**
(`shared/git-worktree-command-capabilities.js`). Orca supports old native git,
WSL git, and SSH git of unknown vintage. Forge targets macOS/Linux with local
git, pins `LC_ALL=C`, and `worktree list --porcelain` has been stable since git
2.7. A probe for a git Forge does not support is a branch that can only rot.

**The worktree lineage graph** (`shared/resolved-worktree-lineage.js`) — edge
validation across repo/host/project boundaries plus instance ids, and an explicit
cycle detector. Forge's parent/child relation lives on `Session`
(`domain/src/session.rs`), which is where the orchestration actually is. A second
graph on `Workspace` would be a second thing to keep consistent, and cycle
detection is only needed because Orca lets the two drift.

**Message-matched force-delete classification**
(`shared/worktree/removal.js`, ~100 lines of prefix constants and matchers shared
between main and renderer, including a distinction between "we watched this PTY
stay alive" and "we could not confirm"). The *distinction* is real and Forge's
`precheck_remove` → `PreconditionFailed` → confirm → `force = true` flow already
carries the same information as structured data (`RemovePrechecks`), which is
strictly better than parsing a string. Do not adopt string matching where a type
already exists.
