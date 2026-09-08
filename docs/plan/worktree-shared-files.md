# Plan — Sharing ignored files between worktrees (alternatives)

Status: **research and options only.** No code in it has been applied. The
decisions of section 12 were closed on 2026-09-04 and the implementation plan
that follows from them is
[worktree-shared-files-implementation.md](./worktree-shared-files-implementation.md). Section 12 lists the decisions that have to be closed
before the implementation plan is written; section 13 says what that plan will
contain once they are.

Scope: what a managed worktree needs on disk in order to actually *run* the
project — `.env`, `node_modules`, `.venv`, `target/`, local certificates, local
settings files — none of which git carries, plus the per-project configuration
surface where the user chooses what gets shared and how.

Related: [worktrees.md](../worktrees.md) (§14, the behaviour that exists),
[plan-branches-and-worktrees.md](../plan-branches-and-worktrees.md) §B4 (where
this gap was first written down), [architecture.md](../architecture.md),
ADR-008 (Git CLI first).

---

## 1. The problem

A managed worktree is created at

```
<worktrees.root>/<project-id>/<slug>          # ~/Library/Application Support/Forge/worktrees/…
```

which is **not** next to the repository. `git worktree add` writes exactly the
tracked content of the branch and nothing else — untracked and ignored files are
by definition not in the object database, so the new checkout starts without:

| Missing | Why the branch cannot run without it |
|---|---|
| `.env`, `.env.local`, `.envrc` | app boots with no database URL / API keys |
| `node_modules/`, `.venv/`, `vendor/` | first command is `command not found` or `ModuleNotFoundError` |
| `target/`, `.next/`, `.turbo/`, `dist/` | not required, but a cold build costs minutes per worktree |
| `*.pem`, `*.p12`, local certs | TLS dev server refuses to start |
| `.claude/settings.local.json`, `.vscode/settings.json`, `.tool-versions` | agent/editor loses its per-project setup |
| local SQLite files, seeded fixtures | app starts and then 500s |

The failure lands on an agent's *first* command in a brand-new worktree, and
nothing in the error points at the missing file — it looks like a bug in Forge.

The user-facing ask is two-sided:

1. **Mechanism** — how a file from the primary checkout becomes available in
   another worktree, at the git and filesystem level.
2. **Configuration** — a per-project section where the user picks *which* files
   are shared and *how*, rather than one global list in a TOML file nobody
   opens.

## 2. What Forge does today

`Daemon::provision_worktree` (`crates/daemon/src/core.rs:2174`), called from
`create_managed_worktree` after `git worktree add`:

- copies every entry of `[worktrees] copy` from the main checkout with
  `copy_into_worktree` (`crates/daemon/src/core.rs:4078`) — relative paths only,
  no `..`, **plain files only** (`core.rs:4097` rejects directories), permissions
  preserved (`core.rs:4110`), a missing source skipped silently;
- runs `[worktrees] setup_script` through `sh -c` from the worktree root with a
  `setup_timeout_secs` budget (default 120 s);
- never fails the worktree: problems become `DaemonNotice`s.

Config lives in `WorktreesConfig` (`crates/daemon/src/config.rs:108`) and is
documented in [config.example.toml](../config.example.toml).

### The four gaps

- **G1 — global, not per-project.** One `copy` list applies to every project the
  user has registered. A Rust project and a Next.js monorepo need different
  lists; today they share one.
- **G2 — no directories.** `node_modules`, `.venv` and `target/` are exactly the
  entries a worktree needs most and exactly the ones `copy` refuses. The refusal
  is right for a byte-for-byte copy (§5, M1) and wrong as a final answer.
- **G3 — no UI, no discovery.** The list can only be edited by hand in
  `config.toml`, requires a daemon restart, and nothing tells the user which
  ignored files their project even has.
- **G4 — creation-time only.** Provisioning runs once, inside `CreateWorktree`.
  A worktree created in a terminal and adopted by `rescan_worktrees` is never
  provisioned; an `.env` that gains a key after the worktree existed never
  reaches it; there is no way to re-run it.

## 3. Git-level facts that constrain the design

1. **`git worktree add` copies no untracked file.** There is no `--include-untracked`,
   no template mechanism, no config knob. Every tool in this space solves it
   outside git's object model.
2. **Worktrees share one common git dir.** `git rev-parse --git-common-dir`
   resolves to the main `.git` from *any* worktree, while `--git-dir` gives the
   per-worktree `.git/worktrees/<name>`. That common dir is the one location
   every worktree of a project agrees on, and it moves and dies with the repo.
3. **`$GIT_COMMON_DIR/info/exclude` is shared by every worktree.** Ignore rules
   can be added there without touching the user's committed `.gitignore` — which
   matters, because injected symlinks otherwise show up as untracked in
   `git status` and in Forge's own dirty indicator.
4. **`git worktree add` fires the `post-checkout` hook** (with the branch-checkout
   flag), and hooks resolve through the shared common dir or `core.hooksPath`.
   That is the only *native* extension point, and it is how the third-party tools
   in §10 provision worktrees created from a terminal.
5. **A branch cannot be checked out in two worktrees.** Already handled:
   `GitError::Conflict` carries the offending path (`git-service/src/worktree.rs`).
   It matters here only because it forces one worktree per branch, which is what
   makes per-worktree dependency state diverge in the first place.
6. **`git status` sees what we inject.** A copied file is untracked; a symlink is
   an untracked symlink; a hardlink is indistinguishable from a regular file.
   Whatever the mechanism, ignore coverage is part of it, or every provisioned
   worktree is permanently "dirty" and `RefreshWorkspaceStatus` reports noise.
7. **`git config --worktree`** exists (with `extensions.worktreeConfig`), so
   per-worktree git configuration is possible — irrelevant for file contents, but
   the right tool if a shared mechanism ever needs a per-worktree marker that
   survives outside our database.

## 4. The design space

Four independent axes. Most of the confusion in this area comes from arguing one
option against another when they sit on different axes.

| Axis | Question | Options |
|---|---|---|
| **M — mechanism** | how does the byte reach the worktree? | copy / symlink / hardlink / CoW clone / generate / env var / mount / sync |
| **S — source of truth** | which copy is authoritative? | main checkout / per-project store in the common git dir / Forge data dir / external (secret manager, package store) |
| **C — rule storage** | where do the rules live and what scopes them? | global TOML / per-project DB row / committed repo file / common-git-dir file |
| **T — timing** | when does provisioning run? | on create / on adopt / on demand / on watch |

## 5. Mechanism alternatives

| # | Mechanism | Disk | Freshness | Isolation | Directories | Verdict |
|---|---|---|---|---|---|---|
| M1 | Copy (today) | full | frozen at creation | total | no (today) | keep, as one of several |
| M2 | Symlink → main checkout | zero | live | none (writes hit main) | yes | offer, not as default |
| M3 | Symlink → shared store | zero | live | none (writes hit the store) | yes | strong default for config |
| M4 | Hardlink | zero | live-ish, breaks silently | none until it breaks | no | reject |
| M5 | CoW clone (`clonefile`/reflink) | ~zero at creation | frozen at creation | total | yes | strong default for dep dirs |
| M6 | Generate (setup script / package manager / secret manager) | full | per-branch correct | total | yes | keep, complements everything |
| M7 | Share through environment variables | zero | live | shared cache, isolated tree | n/a | underrated, cheap win |
| M8 | Bind mount / overlay | zero | live | none | yes | reject (macOS) |
| M9 | Watcher-based sync | full | live | copy-level | yes | reject as default |

### M1 — Copy (what exists)

`std::fs::copy` from the main checkout, mode preserved.

- **For:** total isolation — an agent that rewrites `.env` in a worktree cannot
  corrupt the primary checkout. No aliasing surprises for any tool. Works on
  every filesystem.
- **Against:** frozen at creation (G4); a real duplicate of every secret under
  `~/Library/Application Support/Forge/worktrees` (the tree is `0700`, but the
  count of copies of your production key grows with the number of worktrees);
  unusable for `node_modules`-sized trees.
- **Fits:** small config files where divergence per branch is *desirable* (a
  worktree that needs a different `DATABASE_URL` to run two dev servers at once).

### M2 — Symlink into the main checkout

`ln -s <main>/.env <worktree>/.env`.

- **For:** zero disk, always fresh, works for directories, trivial to implement.
- **Against, and these are the ones that bite:**
  - **the primary checkout becomes load-bearing.** Move, rename or delete it and
    every worktree breaks. It also violates the P4 rule that the main checkout is
    just another workspace — this makes it structurally special.
  - **atomic writes silently sever the link.** Editors and many tools write
    `tmp` then `rename()` over the target. Renaming over a *symlink path*
    replaces the symlink with a regular file: the worktree stops sharing and
    nothing says so. This is the single most common way "symlinked `.env`" fails.
  - **writes go to main.** An agent running `npm install` inside a worktree whose
    `node_modules` is a symlink mutates the primary checkout's dependencies.
  - **resolution semantics.** Bundlers and runtimes differ on whether they
    resolve symlinks (`--preserve-symlinks`, Vite's `preserveSymlinks`, Docker
    bind mounts refusing to follow them out of the context, watchers that do not
    follow links).
- **Fits:** a deliberate "this file is the same file everywhere" choice for
  config, when the user accepts main as the source.

### M3 — Symlink into a per-project shared store

The real file lives in a store — `$GIT_COMMON_DIR/forge/shared/<path>` — and
*every* workspace, including the main checkout, gets a symlink to it.

- **For:** the source of truth is not somebody's checkout, so P4 survives; the
  store dies with the repository and moves with it; removing a worktree removes
  only links; one edit is visible everywhere immediately; directories work.
- **Against:** the first activation has to *move* the user's real `.env` into
  `.git/forge/shared/` and leave a link in its place — an intrusive, must-be-
  reversible, must-be-backed-up operation, and the reason this cannot be a
  silent default. Inherits M2's atomic-write and write-through problems. Puts
  potentially large trees inside `.git`, which surprises anyone who backs up or
  measures that directory.
- **Variant:** store outside the repo, in Forge's data dir keyed by project id.
  Avoids the `.git` weight, loses "moves and dies with the repo", and adds an
  orphan-store cleanup problem.

### M4 — Hardlink

- **For:** no dangling links, no resolution weirdness, zero disk, the file *is*
  the same inode.
- **Against:** the same atomic rewrite that degrades a symlink **silently severs
  a hardlink**, and unlike a symlink there is no observable difference afterwards
  — two files that look identical and drift apart. Same-filesystem only.
  Directories cannot be hardlinked on macOS. **Reject.**

### M5 — Copy-on-write clone

`clonefile(2)` on APFS (`cp -c`), reflink on btrfs/XFS (`cp --reflink=auto`),
falling back to a plain copy.

- **For:** effectively instant and free at creation *including for directories* —
  a 900 MB `node_modules` clones in milliseconds and costs nothing until pages
  diverge. Full isolation afterwards: the agent may `npm install`, rebuild
  `target/`, and never touch another workspace. This is what makes "share
  `node_modules`" safe when symlinking it is not.
- **Against:** same volume only; silently degrades to a full copy elsewhere (the
  fallback must be *reported*, not silent, because a 900 MB surprise is a
  different product); the clone is a snapshot, so a later `npm install` in main
  does not propagate; divergence over time reclaims real disk.
- **Fits:** every large derived directory. Forge is macOS/APFS-first, which is
  precisely where this is best supported.

### M6 — Generate instead of share

Run the thing that produces the file: `pnpm install` (hard-linked from a global
content-addressed store, so it is already near-free), `bun install`
(`clonefile` on macOS), `uv sync`, `mise install`, `direnv` with `source_up`,
`sops -d`, `op inject`, `doppler run`.

- **For:** the only mechanism that is *correct per branch* — a branch that adds a
  dependency gets it, and a branch that removes one does not silently keep it.
  Secrets are never duplicated at rest when they come from a manager. It is also
  the mechanism the ecosystem is already optimising for.
- **Against:** needs the tool installed and, often, the network; the first run
  costs seconds to minutes; a failing script must not block the worktree (already
  the rule today).
- **Fits:** dependencies, always, as either the default or the fallback for M5.

### M7 — Share through environment, not through files

Rather than sharing a directory, point the tool at a shared location:
`CARGO_TARGET_DIR`, `PNPM_STORE_DIR`, `UV_CACHE_DIR`, `TURBO_CACHE_DIR`,
`PLAYWRIGHT_BROWSERS_PATH`, `GOMODCACHE`, `PIP_CACHE_DIR`.

- **For:** no filesystem trickery at all, no aliasing, no drift, and the tools are
  designed for a shared cache. `CARGO_TARGET_DIR` pointed at one directory per
  project turns N cold builds into one warm one. Forge already has the machinery
  to inject environment into sessions (agent profile env,
  `daemon/src/environment.rs`).
- **Against:** per-tool, not general; some caches (cargo's) take a coarse lock,
  so two concurrent builds serialise instead of running in parallel — a
  trade-off, not a bug, but one the UI has to state.
- **Fits:** caches. It belongs in the same configuration section, as a different
  *kind* of share.

### M8 — Bind mount / overlay

`mount --bind`, overlayfs. Linux-only; on macOS it needs macFUSE/bindfs, a
kernel extension and a permission dialog. **Reject.**

### M9 — Watcher-driven sync

Watch the source and re-copy into every worktree on change.

- **Against:** writes into a directory an agent is actively using; bidirectional
  edits have no correct resolution; a permanent watcher per project per file.
  **Reject as a default.** Its sane subset is an explicit **"Sync now"** action
  (§8), which is worth having on its own.

## 6. Source-of-truth alternatives

- **S1 — the main checkout.** Zero setup, breaks P4, dies when the user moves the
  repo. Fine as the *seed* for copy/clone (which is what today's `copy` does);
  questionable as the permanent target of a link.
- **S2 — a store in the common git dir** (`$GIT_COMMON_DIR/forge/shared/`).
  Repo-scoped, worktree-agnostic, survives worktree removal, moves with the repo,
  and is exactly what the prior art converged on (§10). Costs an intrusive
  first migration.
- **S3 — a store in Forge's data dir** keyed by project id. No `.git` weight, no
  migration inside the user's repo, but a store that outlives its repo and has to
  be garbage-collected.
- **S4 — external** (1Password/SOPS/Doppler, pnpm store, cargo registry cache).
  Nothing to store, nothing to sync; needs the tool. Pairs with M6/M7.

These are not exclusive: `.env` can come from S4, `node_modules` from M5 seeded
by S1, `target/` from M7.

## 7. Rule-storage alternatives

- **C1 — global `[worktrees] copy` (today).** Keep as the floor/default. Cannot
  express per-project rules (G1).
- **C2 — per-project rules in SQLite, edited in the GUI.** A `worktree_shares`
  table (or a JSON column on `projects`) plus protocol commands to read, write
  and apply. This is the only option that gives the user the section they asked
  for, since the UI needs a writable, immediately-effective store — `config.toml`
  requires a restart and hand-editing.
- **C3 — a committed file in the repo** (`.forge/worktrees.toml`). Rules travel
  with the project and with the team, are reviewed in PRs, and can differ per
  branch. Costs a file in the user's repository and makes the rules
  branch-dependent — the rules that apply are the ones on the branch you create
  *from*, which is surprising exactly once per team.
- **C4 — an uncommitted file in the common git dir** (`.git/forge/shares.toml`).
  Per clone, shared by every worktree, invisible to the team, survives worktree
  removal, editable by hand. Natural companion to S2.
- **C5 — a precedence chain**: built-in defaults → global `config.toml` →
  committed `.forge/worktrees.toml` (if present) → per-project rules from the
  GUI. Every layer additive, later layers overriding by path. More code, and the
  only shape that satisfies "the team can ship sensible defaults *and* I can
  override them locally".

## 8. Timing alternatives

- **T1 — on create** (today). Necessary, insufficient (G4).
- **T2 — on adopt.** `rescan_worktrees` / `RefreshProject` find worktrees created
  in a terminal. Provisioning them is the difference between "Forge worktrees
  work" and "worktrees work".
- **T3 — on demand.** An explicit *Sync shared files* action per workspace and
  per project, with a preview of what it would do. This is what makes an edited
  `.env` reach existing worktrees without recreating them.
- **T4 — on the git hook.** An installed `post-checkout` hook provisions
  worktrees created outside Forge *at creation time*, without Forge running. The
  most "native" option and the one with the most blast radius: it edits the
  user's repository hooks and it runs when Forge is not there to report failures.
  Offer it, never install it silently.
- **T5 — on watch.** See M9. Reject.

## 9. UI alternatives

- **U1 — a per-project section in Settings.** The obvious home, next to the
  existing sections in `apps/tauri/src/settings/`. Everything is visible at once
  and it is where a user goes to change a rule for the tenth time.
- **U2 — a step in the "New workspace" dialog.** Shows the rules at the moment
  they are about to be applied, with per-creation opt-outs. The best place to
  *notice* the feature exists, the worst place to maintain a list.
- **U3 — both, with one owner.** Settings owns the rules; the creation dialog
  shows a read-only summary ("3 files linked, `node_modules` cloned, `pnpm i`
  will run") with an expander to edit. Recommended.

Whatever the surface, it needs a **discovery** step to be usable: list the
project's ignored entries (`git status --porcelain --ignored=matching -z`, or
`git check-ignore` over a shallow root scan), classify them (secret / dependency
/ build output / cache / editor state) and propose a strategy per class, so the
user checks boxes instead of typing paths. It also needs **per-worktree status**
— shared / stale / diverged / missing / link severed — because with M2/M3 a
silently severed link is the expected failure and the UI is the only place it can
be seen.

## 10. Prior art

- **[git-worktree-share](https://github.com/keithamus/git-worktree-share)** —
  stores shared files in `.git/shared/` with a `.manifest`, symlinks them into
  every worktree, resolves the store through `git rev-parse --git-common-dir`,
  backs up any real file it would overwrite as `<file>.shared-backup`, supports
  directories, and installs an optional `post-checkout` hook so `git worktree add`
  syncs automatically. This is S2 + M3 + T4, and it is the closest thing to a
  reference implementation.
- **[worktree-env-copy](https://github.com/DaniAkash/worktree-env-copy)** — a
  global `post-checkout` hook that copies `.env*` into new worktrees, monorepo-
  aware, chains per-repo hooks. S1 + M1 + T4, deliberately narrow.
- **[git-worktree-sync](https://github.com/fs0414/git-worktree-sync)** — ships
  per-project-type templates of symlink/copy rules; the "presets" idea of §9.
- **[Conductor](https://www.conductor.build/docs/concepts/workspaces-and-branches)**
  — copies allow-listed local files and runs setup scripts per workspace; the
  model Forge already implements, and the reason `[worktrees] copy` exists.
- **[Node/pnpm guidance](https://continuumcode.ai/guides/git-worktree-node-modules/)**
  — symlinking `node_modules` "fails silently when branches do not have identical
  dependencies"; pnpm's hard-linked store and bun's `clonefile` are the sanctioned
  ways to make per-worktree installs cheap. Direct support for "M5/M6 for
  dependencies, never M2/M3".
- **[MindStudio](https://www.mindstudio.ai/blog/git-worktrees-parallel-ai-coding-agents)**
  / **[Verdent on Codex worktrees](https://www.verdent.ai/guides/codex-app-worktrees-explained)**
  — the agent-parallelism framing: provisioning is what separates a worktree an
  agent can use from one it cannot.

## 11. The shape that follows from all of this

Not a decision, a reading of the table: **no single mechanism is right for every
file, so the unit of configuration is a rule, not a list.**

```
share rule := { path, strategy, source, scope }
strategy   := copy | link | clone | run | env
```

with defaults proposed per detected class:

| Class | Example | Proposed strategy | Why |
|---|---|---|---|
| Secret / config | `.env*`, `*.pem`, `.npmrc` | `link` (S2 store), `copy` offered | one edit reaches every worktree; small; rarely rewritten by the app |
| Dependencies | `node_modules/`, `.venv/`, `vendor/` | `clone` (M5) → `run` fallback (M6) | isolation is required (per-branch lockfiles); CoW makes it free |
| Build output | `target/`, `.next/` | `env` (M7) where the tool supports it, else nothing | shared cache is the supported path; a copy is worthless |
| Cache | pnpm store, `~/.cargo` | `env` (M7) | already designed to be shared |
| Editor / agent state | `.claude/settings.local.json`, `.vscode/` | `copy` | must not be aliased across worktrees |

Everything else in this document is about which of those defaults we ship, where
the rules are stored, and when they run.

## 12. Decisions — closed 2026-09-04

Each was posed with a recommendation; the answer is recorded in **bold** under
it. `strategy = env` and the `post-checkout` hook were the two recommendations
that were *not* taken as-is: `link` was pulled into v1 (D1) and the hook stayed
out (D4).

- **D1 — mechanism set.** Ship all of `copy`/`link`/`clone`/`run`/`env`, or a
  narrower first cut?
  *Recommended:* `copy` + `clone` + `run` first, `link` only if D2 lands, `env` last.
  **Closed: `copy` + `clone` + `run` + `link` in v1; `env` designed for, not built.**
- **D2 — source of truth for `link`.** Main checkout (S1), common-git-dir store
  (S2), or Forge data dir (S3)?
  *Recommended:* S2, with the move-and-backup migration behind an explicit
  confirmation, and never as an automatic default. **Closed: S2, as recommended.**
- **D3 — where rules live.** C2 alone, or the C5 chain with a committed
  `.forge/worktrees.toml`?
  *Recommended:* C2 now (DB + GUI + protocol), designed so C3 can be imported
  later; keep C1 as the global default layer. **Closed: C2, as recommended.**
- **D4 — timing.** T1+T2+T3 (create, adopt, on demand) vs adding T4 (git hook)?
  *Recommended:* T1+T2+T3 now; T4 as an explicit, reversible opt-in later, since
  it writes into the user's repository. **Closed: T1+T2+T3; no hook in v1.**
- **D5 — directories.** Lift the "plain files only" rule for `clone` only, or for
  `copy` too?
  *Recommended:* `clone` yes; `copy` of a directory only above an explicit size
  confirmation, because a fallback copy of `node_modules` on a non-APFS volume is
  a multi-hundred-megabyte surprise.
- **D6 — secrets.** Do we treat `.env`-class files differently: refuse them
  outside the data dir, warn, mask them in the UI, log them by path only?
  *Recommended:* classify them, warn once when a strategy duplicates them at
  rest, and never print their contents in notices or logs.
- **D7 — status and ignore hygiene.** Do we write ignore entries to
  `$GIT_COMMON_DIR/info/exclude` for what we inject (§3.3), and do we surface a
  per-worktree share status (§9)?
  *Recommended:* yes to both; without them every provisioned worktree reads as
  dirty and every severed link is invisible.
- **D8 — scope of "worktree" here.** Rules apply only to managed worktrees, or
  also to the main checkout and to adopted external ones?
  *Recommended:* every workspace of the project, P4 — the main checkout is not a
  special case, it is just the one that usually already has the files.

D5–D8 were taken as recommended: directories for `clone` (and for `copy` past a
size confirmation), secrets classified and never printed, ignore hygiene plus
per-workspace status, and rules scoped to every workspace of the project (P4).

## 13. What the implementation plan contains

Written against these seams, in this order — the detail is in
[worktree-shared-files-implementation.md](./worktree-shared-files-implementation.md):

1. **Domain + protocol** — the `ShareRule` type, `GetProjectShares` /
   `SetProjectShares` / `PreviewShares` / `ApplyShares` requests, and the
   `SharesApplied` / per-workspace share-status events.
2. **Persistence** — a migration for the per-project rules (following the
   `agent_profiles` precedent, `crates/persistence/src/migrations.rs`) and its
   repository.
3. **`git-service` / a new provisioning module** — `clonefile`/reflink with
   reported fallback, link-and-store with backup, safe path validation (no `..`,
   no absolute, nothing inside `.git`, no symlink escape), ignore-file hygiene.
4. **Daemon** — `provision_worktree` rewritten as "apply the project's rules to a
   workspace", called from create (T1), adopt (T2) and the new explicit request
   (T3); notices instead of failures, unchanged.
5. **GUI** — the per-project settings section (U1) with detection and presets, the
   read-only summary in the New workspace dialog (U3), and the per-workspace
   share status.
6. **Tests** — real-git integration tests per strategy, a non-CoW fallback test, a
   severed-link detection test, and a scenario test that creates a worktree in a
   terminal and has Forge adopt *and* provision it.
