# Explore — harness state paths are resolved against cwd, not against the state root

**Status:** investigation only. Nothing in this document has been applied.
**Scope:** `.claude/agents/*.md`, `.claude/skills/*/SKILL.md`, `harness/src/cli.ts`,
`scripts/harness`, `.claude/hooks/session-guard.sh`, `harness/test/harness.test.ts`.
**Explicitly out of scope:** `apps/tauri/` (two other agents are editing it).

---

## 1. The defect, restated from evidence

`harness/src/harness.ts:31-41` resolves `ROOT` as `FORGE_HARNESS_ROOT` →
`git rev-parse --path-format=absolute --git-common-dir` → own checkout. Every
consumer of the TypeScript (`cli.ts`, `validate.ts`, `events.ts`) therefore
reads and writes the **main** checkout's `harness/`. Verified from a worktree:

```
$ (cd ".../worktrees/.../test3" && bun harness/src/cli.ts doctor 7)
harness:  harness
checkout: /Users/.../worktrees/.../test3 (state shared from /Users/kaonashi/dev/forge-node)
```

The Rust end agrees: `crates/harness-service/src/lib.rs` takes `project_root`
as a parameter and never looks at cwd; `crates/daemon/src/core.rs:4702`
(`harness_root_in`) maps a workspace path to `Project::root_path`, and both the
PTY path (`core.rs:3330`) and the job path (`jobs.rs:300`) export that as
`FORGE_HARNESS_ROOT`.

The **prose** does not agree. All seven instruction files address the state with
bare relative paths, so an agent whose cwd is a worktree writes beside itself.
Three of three managed worktrees currently show `M harness/features.json` —
an agent edited the worktree's frozen tracked copy in every single run.

### A second failure the brief did not name

`events_7.jsonl` did **not** land in the main checkout because
`scripts/harness event` resolved `ROOT`. Compare:

```
main   {"ts":"2026-08-29T18:04:37.947861Z","type":"feature_registered",...}   3 lines
test3  {"ts":"2026-08-29T00:00:00.000Z","type":"feature_registered",...}      5 lines
```

The main copy has real microsecond RFC3339 stamps — that is
`harness_service::append_event` (the daemon), fired by `RegisterFeature` /
`advance`. The worktree copy has synthetic `T00:00:0N.000Z` stamps: the agent
**hand-wrote the JSONL** instead of calling `scripts/harness event`. So the
fix must also say, in the prose, that the event log is appended *only* through
the CLI. Otherwise the one path that resolves `ROOT` correctly keeps getting
bypassed.

---

## 2. Every path reference in the seven files, classified

`W` = the agent writes it · `R` = the agent reads it · `P` = printed/quoted only
(no filesystem access) · `C` = a command, correctly checkout-local.

Only `W` and `R` are load-bearing. `C` rows must stay relative: `scripts/harness`,
`./init.sh` and `scripts/dev` act on the *code in your checkout*, and
`scripts/harness` resolves the state root by itself.

### `.claude/agents/spec-author.md`

| line | reference | class |
|------|-----------|-------|
| 10 | `harness/features.json` | R |
| 12 | `harness/specs/<id>-<slug>/requirements.md` | **W** |
| 13 | `harness/specs/<id>-<slug>/design.md` | **W** |
| 14 | `harness/specs/<id>-<slug>/tasks.md` | **W** |
| 24 | `harness/features.json` | R |
| 28 | `harness/progress/explore_<id>_*.md` | R |
| 37 | `./init.sh` | C |
| 38 | `harness/progress/gate_<id>.md` | **W** |
| 46 | `**Spec:** harness/specs/<id>-<slug>/` (gate template body) | P |
| 52-53 | `scripts/harness event …` | C |
| 52 | `--data '{"path":"harness/specs/…"}'` | P (payload) |
| 56 | `harness/features.json` | **W** |
| 111 | `spec_ready -> harness/specs/…` (return line) | P |
| 115 | `blocked -> harness/progress/spec_<id>.md` | **W** (implies writing it) |

### `.claude/agents/implementer.md`

| line | reference | class |
|------|-----------|-------|
| 10 | `harness/specs/<id>-<slug>/` | R |
| 19 | `harness/progress/context_<id>.md` | R |
| 22 | `harness/features.json` | **W** |
| 23 | `harness/progress/current_<id>.md` | **W** |
| 27 | `scripts/harness event <id> impl_started` | C |
| 33 | `scripts/dev check` | C |
| 36 | `harness/features.json` | **W** |
| 39 | `scripts/harness event … gate_failed` | C |
| 43 | `current_<id>.md` (bare) | **W** |
| 49 | `scripts/harness event … gate_passed` | C |
| 54 | `harness/progress/impl_<id>.md` | **W** |
| 60 | `scripts/harness event … impl_done` + payload | C + P |
| 65 | `harness/progress/review_<id>.md` | R |
| 66 | `impl_<id>.md` (bare) | **W** |
| 93 | `current_<id>.md` (bare) | **W** |
| 104, 108 | return lines | P |

### `.claude/agents/reviewer.md`

| line | reference | class |
|------|-----------|-------|
| 3 | `harness/CHECKPOINTS.md` (frontmatter description) | P |
| 18 | `harness/CHECKPOINTS.md` | R — **checkout-local, correct as-is** (tracked doc, matches the code under review) |
| 18-19 | the feature's spec files | R (shared) |
| 20 | `harness/progress/impl_<id>.md` | R |
| 26 | `scripts/harness event … review_started` | C |
| 32 | `./init.sh` | C |
| 34-35 | `crates/{client,ui}/Cargo.toml` | R — checkout-local, correct |
| 37 | `migrations.rs` | R — checkout-local, correct |
| 44 | `harness/progress/review_<id>.md` | **W** |
| 48 | `scripts/harness event … review_verdict` + payload | C + P |
| 61 | `./init.sh` inside the verdict template | P |
| 94, 98 | return lines | P |

### `.claude/agents/committer.md`

| line | reference | class |
|------|-----------|-------|
| 14 | `harness/features.json` | R |
| 17 | `harness/progress/review_<id>.md` | R |
| 18 | `scripts/harness gate` | C |
| 19 | `git status --porcelain` | C |
| 24 | `harness/specs/<id>-<slug>/requirements.md` | R |
| 25 | `harness/progress/impl_<id>.md` | R |
| 32 | `scripts/harness event … feature_committed` | C |

**Extra defect in this file, line 26-27** (`Stage only the paths the implementer
listed, plus harness artefacts for that feature`): `git add` cannot stage a path
that lives outside the worktree. From a worktree the harness artefacts are in a
*different working tree of the same repository* and need their own
`git -C "$ROOT" add harness/...` + commit, or they silently never get staged.
Prefixing the path is not enough here; the instruction needs a sentence.

### `.claude/skills/feature/SKILL.md`

| line | reference | class |
|------|-----------|-------|
| 6 | `allowed-tools` patterns | C (permission grammar — see §4) |
| 25 | `harness/**` | P |
| 36-37 | `harness/progress/explore_1_server.md` — the **verbatim template** the lead pastes into every `Explore` prompt | **W** (this is what orphaned `explore_7_themes.md`) |
| 43, 45 | `scripts/harness gate --fast`, `./init.sh --fast`, `scripts/harness status` | C |
| 53 | `harness/features.json` | **W** |
| 71 | `scripts/harness event … feature_registered` | C |
| 74 | `harness/progress/current_<id>.md` | **W** |
| 79 | `scripts/harness from-issue` | C |
| 91 | `harness/progress/explore_<id>_<topic>.md` | **W** (via subagents) |
| 95 | `scripts/harness event … explore_done` + payload | C + P |
| 109 | `harness/specs/<id>-<slug>/` (report to human) | P |
| 110 | `harness/progress/gate_<id>.md` (report to human) | P |

### `.claude/skills/feature-go/SKILL.md`

| line | reference | class |
|------|-----------|-------|
| 6 | `allowed-tools` | C |
| 18, 19 | `scripts/harness gate --fast`, `scripts/harness show` | C |
| 22 | `harness/specs/<id>-<slug>/` (precondition) | R |
| 23 | `harness/progress/gate_<id>.md` (precondition) | R |
| 24 | `git rev-parse HEAD` | C |
| 30 | `scripts/harness event … human_gate_resolved` | C |
| 35 | `harness/progress/context_<id>.md` | **W** |
| 37 | `features.json` (`spec_raw` verbatim) | R |
| 38 | `explore_<id>_*.md` | R |
| 45 | `scripts/harness event … context_bundled` + payload | C + P |
| 54 | `harness/progress/impl_<id>.md` (relayed to implementer) | **W** |
| 68 | `harness/features.json` | **W** |
| 72 | `harness/progress/current_<id>.md` | **W** |
| 76 | `harness/features.json` | **W** |
| 77 | `scripts/harness event … feature_done` | C |
| 78-79 | `harness/progress/current_<id>.md` → `harness/progress/history.md` | **R + W + delete** |
| 80 | `scripts/harness gate` | C |

### `.claude/skills/feature-status/SKILL.md`

| line | reference | class |
|------|-----------|-------|
| 5 | `allowed-tools` | C |
| 10-12 | `scripts/harness status/show/resume/timeline` | C |
| 20 | `events_<id>.jsonl` (bare) | R |
| 21 | `harness/progress/` | R |

**Totals:** 41 `harness/...` mentions; **31 load-bearing** (18 W, 13 R), 10
print-only, plus 22 `C` command rows that must stay relative.

---

## 3. Is `FORGE_HARNESS_ROOT` reliably available? — No.

- Set: sessions the daemon starts through the PTY path
  (`core.rs:3330`, only when `harness_root_in` finds the cwd among
  `inner.workspaces`) and headless jobs (`jobs.rs:299-301`, same condition).
- **Unset:** a plain terminal, `claude` launched by hand, a `cd` into a worktree
  from an existing shell, any session whose cwd is not a registered workspace,
  and CI. It is unset in this very investigation session.

`harness.ts:32` already treats it as an *override*, not a requirement, and falls
back to `git rev-parse --git-common-dir`. So the variable is the right contract
between daemon and harness, and the wrong contract for markdown prose:
`${FORGE_HARNESS_ROOT:-$(pwd)}` — the shape
`crates/daemon/tests/scenario_harness_cycle.rs:261` uses for its *fixture* —
degrades to `$(pwd)`, which in a worktree is exactly the bug.

**Any fix must go through `harnessRoot()`, which already handles both cases.**

---

## 4. Is there already a supported way to ask for the root? — No.

Checked exhaustively:

- `scripts/harness` — a 20-line `exec bun harness/src/cli.ts "$@"` wrapper. No `root`.
- `harness/src/cli.ts` — commands are `status, list, next, active, show, doctor,
  watch, resume, timeline, event, from-issue, validate, gate`. The only place
  `ROOT` reaches stdout is `cmdDoctor` (line 424), and only
  **conditionally** (`if (CHECKOUT !== ROOT)`) and embedded in a prose sentence:
  `checkout: <x> (state shared from <y>)`. Not parseable, not always present.
- `harness/src/harness.ts` — `ROOT` is a TypeScript `export`, reachable only by
  importing the module. No CLI surface.

### Decision: add `scripts/harness root`, do not teach seven files a shell fallback

| option | verdict |
|--------|---------|
| **A.** Inline `$(git rev-parse --path-format=absolute --git-common-dir)` in 31 places | **Rejected.** Duplicates the resolution rule 31 times, silently ignores `FORGE_HARNESS_ROOT` (so the daemon and the agent would disagree the moment the daemon's answer differs), and returns `<root>/.git` — every site would also have to strip the suffix and handle the bare-repo case that `harness.ts:39` handles once. |
| **B.** `${FORGE_HARNESS_ROOT:-$(pwd)}` everywhere | **Rejected.** Half a fix: correct only in the case that is already correct. Outside the daemon it degrades to the worktree, which is the defect. |
| **C.** `scripts/harness root` + prose that resolves once | **Chosen.** One implementation (`harnessRoot()`), already the daemon's own answer, already the fallback for a non-git copy. Seven files learn *one command*, not one algorithm. Matches this repo's stated preference — `git_service::run_git`, `theme tokens`, `crates/agents` are all the same shape: one place writes the rule down. |

**Idiom for the prose:** run `scripts/harness root` as **its own Bash call**,
then use the literal absolute path it printed in every subsequent Read / Write /
Edit and in every Bash command. Two reasons this beats inlining `$(…)`:

1. Each Bash call is a fresh shell — an exported `HARNESS_ROOT` does not survive
   to the next call, so a "set it once" instruction would silently break.
2. Permission grammar. `.claude/settings.json` allows `Bash(scripts/harness*)`,
   which matches the bare `scripts/harness root` call. It does **not** match
   `cat "$(scripts/harness root)/harness/progress/impl_7.md"`, because the
   pattern anchors at the start of the command string. Resolving in a separate
   call keeps the permission surface exactly as it is today — no
   `settings.json` change is needed for this fix.

Inlining `$(scripts/harness root)` in one command stays a documented fallback
for a one-shot; the primary instruction is resolve-then-paste.

---

## 5. The other entry points

| entry point | verdict |
|-------------|---------|
| `scripts/harness` | Correct. Pure `exec` into `cli.ts`; the `cd "$(dirname "$0")/.."` only reaches the checkout, and `ROOT` is resolved after that. |
| `init.sh` | Correct **by design**, with one nuance. Every path it touches is checkout-local because it gates the code in *this* checkout. Its only state touch is block 3, `bun harness/src/validate.ts`, which resolves `ROOT` itself — this is what reported feature 7's missing spec. Block 2's `[ ! -e harness/features.json ]` is a checkout-local existence check, which is what it means to ask ("is this repo harnessed"). Leave it. |
| `.claude/hooks/fmt-and-check.sh` | Correct. `cd "${CLAUDE_PROJECT_DIR:-…}"` then only `crates/<pkg>` in that checkout. Never touches `harness/`. |
| `.claude/hooks/session-guard.sh` | **Bug, same class.** `cd`s into `CLAUDE_PROJECT_DIR` (the worktree), then lines 39-41 build `NOTE="harness/progress/current_$ID.md"` **relative** and `grep` it. `validate.ts` and `cli.ts active` on lines 24 and 32 resolve `ROOT` correctly, so the guard reads the *shared* feature list and then checks the *worktree's* note — the exact split. It will nag about a note that exists, in the main checkout, for a feature it correctly identified. Fix listed below. |
| `crates/harness-service/` | Correct. Every function takes `project_root: &Path`; `FEATURES` is joined onto it. Module doc says so explicitly. No cwd anywhere except `Command::current_dir(project_root)` for `gh` and `bun`. |
| `crates/daemon/src/{core.rs,jobs.rs}` | Correct. `harness_root_in` → `Project::root_path`. |
| `crates/daemon/tests/scenario_harness_cycle.rs:261` | Correct **for a fixture**. `${FORGE_HARNESS_ROOT:-$(pwd)}` is a fake agent asserting the daemon's export; it is not a template for prose. Leave it. |
| `harness/test/harness.test.ts` | **Latent bug.** `run()` (line 105) inherits the parent environment. Inside any daemon-started session `FORGE_HARNESS_ROOT` is set, `harnessRoot()` honours it before anything else, and every fake repo resolves to the real checkout. Measured: **25 of 34 tests fail**, and the `event appends to the jsonl log` case *wrote `harness/progress/events_9.jsonl` into the real repository*. (I removed that stray file; `git status` is back to what it was.) So the suite is currently unrunnable — and destructive — from exactly the environment the harness runs in. |

---

## 6. `harness/features.json` tracked in git — deliberate, and gitignoring is not available

**Verdict: leave it tracked. This is not a fixable trap; it is a consequence of
git's design, and the mitigation is the path fix, not an ignore rule.**

Evidence it is deliberate:

- `git ls-files harness/` tracks `features.json` **and** `progress/*` **and**
  `specs/*/` — 24 state files. This is a consistent choice, not an oversight.
- `init.sh` block 2 hard-requires `harness/features.json` to exist, and
  `harness/CHECKPOINTS.md` C1 restates it. A fresh clone with the file ignored
  fails the gate immediately.
- `AGENTS.md` § Harness: "The harness never commits, pushes, or adds a
  dependency: it leaves a reviewable diff in the working tree." The tracked copy
  is meant to move only when a human commits it.

Evidence gitignoring is **not** an option:

- `.gitignore` is repository-wide and `.git/info/exclude` lives in the *common*
  dir (`git rev-parse --git-dir` in a worktree is `.git/worktrees/<name>`, but
  `info/exclude` is not per-worktree). There is no "track in main, ignore in
  worktrees" — every rule applies to the main checkout too.
- An ignore rule has no effect on an already-tracked file. Untracking it means
  `git rm --cached`, which deletes it from the main checkout on the next
  `git checkout` of a commit that predates the removal — the worst possible
  outcome for the file the whole gate depends on.

The trap is real but its shape is different from what an ignore would address.
The worktrees `test` and `test1` sit on commit `985d382`, whose `features.json`
holds only features 1-4 and an older schema (no `gate_attempts`, no
`workspace_id`). An agent that reads the relative path there gets a *different
world*, not a stale one. That is fixed by never reading the relative path.

**Optional, non-blocking mitigation (not part of this change):** when
`CHECKOUT !== ROOT` and `<CHECKOUT>/harness/features.json` differs from
`<ROOT>/harness/features.json`, have `validate` print a `[WARN]` naming the
shadow copy. Cheap, informative, and does not touch git.

---

## 7. Features 4, 5, 6, 7 — what is actually split

| id | `workspace_path` | in a worktree? | state |
|----|------------------|----------------|-------|
| 4 | `/Users/kaonashi/dev/forge-node` | **No — the main checkout** | Unaffected. `gate_4.md`, `current_4.md`, `explore_4_scope.md`, `specs/4-test/` are all in main and tracked. The identical copies visible in `test3/harness/progress/` are a snapshot, see below. |
| 5 | `.../worktrees/…/test` | Yes | **Orphaned, but superseded.** `gate_5.md` and `specs/5-add-suport-for-grokai/{requirements,design,tasks}.md` exist **only** in the worktree; main has just `events_5.jsonl`. The worktree's own `features.json` says `spec_ready`; main says `pending`. Its note went to the worktree's shared `progress/current.md`, not `current_5.md`. |
| 6 | `.../worktrees/…/test1` | Yes | **Already recovered.** `explore_6_test.md` and `specs/6-test/*` are byte-identical in main and in `test1` (`diff -rq` clean); `gate_6.md` exists only in main. All three are committed at `53a321b`. A duplicate shadow remains in `test1` and its `features.json` is still `M`. |
| 7 | `.../worktrees/…/test3` | Yes | **One orphan left.** The five files were copied to main as stated. `harness/progress/explore_7_themes.md` (2101 bytes) was **not** — it exists only in `test3`. And `events_7.jsonl` diverges as described in §1. |

Do **not** recover feature 5's spec. `events_5.jsonl` in main ends with
`{"type":"human_gate_resolved","gate":"spec_approval","decision":"revise"}`, and
`HarnessAdvanceAction::ReviseSpec` sets the status back to `pending`
(`crates/harness-service/src/lib.rs`). The worktree draft is a spec the human
already sent back; copying it in would restore a superseded document.

`explore_7_themes.md` is the one worth recovering, and it is the human's call —
it is referenced by nothing that `validate` checks, so leaving it costs only the
research it contains:

```bash
cp "/Users/kaonashi/Library/Application Support/Forge/worktrees/01a02a70-b898-7eb0-91a8-42253ac1ec02/test3/harness/progress/explore_7_themes.md" \
   /Users/kaonashi/dev/forge-node/harness/progress/
```

**Unexplained, benign:** `test3/harness/progress/` contains a full copy of
main's 20 progress files, all stamped `Aug 29 13:03` (worktree creation).
`[worktrees] copy` is empty in this install (no `~/.config/forge/config.toml`;
`config.rs:216` defaults to `Vec::new()`) and `copy_into_worktree` copies files,
never directories — so no code path in this repository produced it. Most likely
a manual copy. Not a defect; noting it so the next reader does not chase it.

---

## 8. The fix — file-by-file edit list

### 8.1 `harness/src/cli.ts` — add the single source of truth

`ROOT` is already imported at line 17. No new import.

**(a)** Insert after `cmdActive` (after line 197, before `async function cmdNext`):

```ts
/**
 * The checkout that owns the harness state — absolute, one line, nothing else.
 *
 * The one supported way to ask the question from outside this module. A
 * worktree's own `harness/` is a frozen copy from its commit, so an agent
 * standing in one has to prefix every state path with this answer instead of
 * writing beside itself. `harnessRoot()` resolves it once — `FORGE_HARNESS_ROOT`
 * first, then `git rev-parse --git-common-dir` — and this prints that, so
 * nothing has to re-derive the rule.
 */
function cmdRoot(): number {
  console.log(ROOT);
  return 0;
}
```

**(b)** In `main`'s switch, insert after `case "active":` (line 535-536):

```ts
    case "root":
      return cmdRoot();
```

**(c)** `USAGE` (line 496): add `root` to the command list in the first line —

```
usage: scripts/harness [-h] {status,list,next,active,root,show,doctor,watch,resume,timeline,event,from-issue,validate,gate} ...
```

and after the `active` line (line 505) add:

```
  root              absolute path of the checkout that owns harness/ (worktree-safe)
```

**(d)** `cmdDoctor` (line 421): make the root unconditional. Replace

```ts
  console.log(`harness:  ${rel(HARNESS)}`);
```

with

```ts
  console.log(`root:     ${ROOT}`);
  console.log(`harness:  ${rel(HARNESS)}`);
```

(The `if (CHECKOUT !== ROOT)` line 424 stays: it names the *difference*, which
is the interesting part.)

### 8.2 `scripts/harness` — document it

In the header comment, after the `scripts/harness active` line (line 15), add:

```
#   scripts/harness root         # absolute path of the checkout that owns harness/
```

No permission change: `.claude/settings.json` already allows
`Bash(scripts/harness*)`, and every skill's `allowed-tools` already carries
`Bash(${CLAUDE_PROJECT_DIR}/scripts/harness*)`.

### 8.3 `.claude/hooks/session-guard.sh` — resolve the note against the root

Replace lines 39-41:

```sh
    NOTE="harness/progress/current_$ID.md"
    [ -f "$NOTE" ] || NOTE="harness/progress/current.md"
```

with:

```sh
    # The note lives with the rest of the state, in the checkout that owns it —
    # this session may be closing in a worktree, whose harness/ is a frozen copy
    # from its commit. `cli.ts active` above already answered from that root.
    ROOT=$(bun harness/src/cli.ts root 2>/dev/null) || ROOT=""
    [ -n "$ROOT" ] || ROOT=$(pwd -P)
    NOTE="$ROOT/harness/progress/current_$ID.md"
    [ -f "$NOTE" ] || NOTE="$ROOT/harness/progress/current.md"
```

The `REASON` string on lines 46-48 already interpolates `$NOTE`; it now names an
absolute path, which is strictly better for the agent that has to act on it.

Leave line 21 (`[ -f harness/features.json ] || exit 0`) alone — it asks "is
this repository harnessed at all", and every checkout carries the tracked file.

### 8.4 The seven instruction files

Each gets **(i)** one shared preamble block and **(ii)** the `W`/`R` rows from
§2 rewritten as `$ROOT/harness/...`. `C` rows are not touched.

**(i) The preamble block.** Insert verbatim into all seven files (placement per
file below):

````markdown
## Where the harness state lives

The harness is one state machine per **repository**. `harness/features.json`,
`harness/progress/` and `harness/specs/` live in the checkout that owns them —
which is **not** necessarily the one you are standing in. A git worktree carries
a frozen copy of those files from its commit, and anything you write there is
invisible to Forge, to `scripts/harness` and to `bun harness/src/validate.ts`.

Resolve it once, as your first action, in its own Bash call:

```bash
scripts/harness root
```

Below, `$ROOT` means the absolute path that command printed. Paste that literal
path into every Read / Write / Edit and into every shell command. Do not export
it: each Bash call is a fresh shell and the variable will not survive.

`scripts/harness`, `./init.sh` and `scripts/dev` stay **relative** on purpose —
they act on the code in *your* checkout, and `scripts/harness` resolves the
state root by itself.

Append to the event log **only** through `scripts/harness event`. Never write
`events_<id>.jsonl` by hand: the CLI is what puts the line in the right file
with a real timestamp.
````

**(ii) Per-file path rewrites.**

#### `.claude/agents/spec-author.md`
Insert the preamble after line 17 (after the "You do not write application
code…" paragraph), before `## Protocol`. Then:

| line | old | new |
|------|-----|-----|
| 10 | `` `harness/features.json` `` | `` `$ROOT/harness/features.json` `` |
| 12 | `` - `harness/specs/<id>-<slug>/requirements.md` `` | `` - `$ROOT/harness/specs/<id>-<slug>/requirements.md` `` |
| 13 | `design.md` line | prefix `$ROOT/` |
| 14 | `tasks.md` line | prefix `$ROOT/` |
| 24 | `` in `harness/features.json` `` | `` in `$ROOT/harness/features.json` `` |
| 28 | `` `harness/progress/explore_<id>_*.md` `` | `` `$ROOT/harness/progress/explore_<id>_*.md` `` |
| 38 | `` Write `harness/progress/gate_<id>.md` `` | `` Write `$ROOT/harness/progress/gate_<id>.md` `` |
| 56 | `` in `harness/features.json` `` | `` in `$ROOT/harness/features.json` `` |
| 115 | `` blocked -> harness/progress/spec_<id>.md `` | keep the *return line* relative (it is P), but add to step 9 or the Hard rules: "the blocker note is written to `$ROOT/harness/progress/spec_<id>.md`" |

Leave lines 46, 52, 111 as-is (P/C).

#### `.claude/agents/implementer.md`
Insert the preamble after line 15 (after the "Worktree isolation" paragraph) —
this agent declares `isolation: worktree`, so it is *guaranteed* to be standing
in one. Then:

| line | old | new |
|------|-----|-----|
| 10 | `` a `harness/specs/<id>-<slug>/` `` | `` a `$ROOT/harness/specs/<id>-<slug>/` `` |
| 19 | `` `harness/progress/context_<id>.md` `` | prefix `$ROOT/` |
| 22 | `` `harness/features.json` `` | prefix `$ROOT/` |
| 23 | `` `harness/progress/current_<id>.md` `` | prefix `$ROOT/` |
| 36 | `` `harness/features.json` `` | prefix `$ROOT/` |
| 43 | `` `current_<id>.md` `` | `` `$ROOT/harness/progress/current_<id>.md` `` |
| 54 | `` `harness/progress/impl_<id>.md` `` | prefix `$ROOT/` |
| 65 | `` `harness/progress/review_<id>.md` `` | prefix `$ROOT/` |
| 66 | `` `impl_<id>.md` `` | `` `$ROOT/harness/progress/impl_<id>.md` `` |
| 93 | `` `current_<id>.md` `` | `` `$ROOT/harness/progress/current_<id>.md` `` |

Leave 27, 33, 39, 49, 60, 104, 108 (C/P).

#### `.claude/agents/reviewer.md`
Insert the preamble after line 14 (after the invariant-skills paragraph). Then:

| line | old | new |
|------|-----|-----|
| 18 | `` Read `harness/CHECKPOINTS.md`, `AGENTS.md` and the feature's full spec `` | `` Read `harness/CHECKPOINTS.md` and `AGENTS.md` **from your own checkout** (they describe the code you are reviewing), and the feature's full spec from `$ROOT/harness/specs/<id>-<slug>/` `` |
| 20 | `` `harness/progress/impl_<id>.md` `` | prefix `$ROOT/` |
| 44 | `` Write the verdict in `harness/progress/review_<id>.md` `` | `` Write the verdict in `$ROOT/harness/progress/review_<id>.md` (heredoc to the absolute path) `` |

Leave 3, 26, 32, 34-38, 48, 61, 94, 98.

#### `.claude/agents/committer.md`
Insert the preamble after line 11. Then:

| line | old | new |
|------|-----|-----|
| 14 | `` `harness/features.json` `` | prefix `$ROOT/` |
| 17 | `` `harness/progress/review_<id>.md` `` | prefix `$ROOT/` |
| 24 | `` `harness/specs/<id>-<slug>/requirements.md` `` | prefix `$ROOT/` |
| 25 | `` `harness/progress/impl_<id>.md` `` | prefix `$ROOT/` |

And **replace step 2** (lines 26-27) to fix the cross-tree staging defect:

```markdown
2. Stage only the paths the implementer listed. If `$ROOT` is not your own
   checkout you are in a worktree, and `git add` cannot reach the harness
   artefacts: they live in another working tree of the same repository. Commit
   the code here, and leave the artefacts to the human — say so in your final
   line. Never `git -C "$ROOT" commit` on the human's behalf.
```

#### `.claude/skills/feature/SKILL.md`
Insert the preamble after line 26 (after `## Hard rules of the role`, before
`## Anti-broken-telephone rule`). Then:

| line | old | new |
|------|-----|-----|
| 36-37 | the example prompt's `harness/progress/explore_1_server.md` (both occurrences) | `$ROOT/harness/progress/explore_1_server.md`, and add to the example: *"Resolve `$ROOT` with `scripts/harness root` before you paste this — the subagent gets the literal absolute path, never the placeholder."* |
| 53 | `` Add an entry to `harness/features.json` `` | prefix `$ROOT/` |
| 74 | `` `harness/progress/current_<id>.md` `` | prefix `$ROOT/` |
| 91 | `` write to `harness/progress/explore_<id>_<topic>.md` `` | prefix `$ROOT/` |

Line 36-37 is the single highest-value edit in the whole change: it is the
template that is pasted verbatim into every `Explore` prompt, and it is what put
`explore_7_themes.md` in the worktree.

Leave 6, 25, 43, 45, 71, 79, 95, 109, 110.

#### `.claude/skills/feature-go/SKILL.md`
Insert the preamble after line 13 (after the "The human approved…" paragraph),
before `## Protocol`. Then:

| line | old | new |
|------|-----|-----|
| 22 | `` `harness/specs/<id>-<slug>/` has the three files `` | prefix `$ROOT/` |
| 23 | `` `harness/progress/gate_<id>.md` exists `` | prefix `$ROOT/` |
| 35 | `` Write `harness/progress/context_<id>.md` `` | prefix `$ROOT/` |
| 37 | `` verbatim from `features.json` `` | `` verbatim from `$ROOT/harness/features.json` `` |
| 38 | `` each `explore_<id>_*.md` `` | `` each `$ROOT/harness/progress/explore_<id>_*.md` `` |
| 54 | `` write to `harness/progress/impl_<id>.md` `` | prefix `$ROOT/` — and add "pass the implementer the resolved `$ROOT`, it runs with `isolation: worktree`" |
| 68 | `` `harness/features.json` `` | prefix `$ROOT/` |
| 72 | `` `harness/progress/current_<id>.md` `` | prefix `$ROOT/` |
| 76 | `` `harness/features.json` `` | prefix `$ROOT/` |
| 78-79 | `` `harness/progress/current_<id>.md` `` → `` `harness/progress/history.md` `` | prefix both with `$ROOT/` |

Leave 6, 18, 19, 24, 30, 45, 77, 80.

#### `.claude/skills/feature-status/SKILL.md`
Read-only skill; smallest edit. Insert a two-line note after line 8:

```markdown
Every path below is under the checkout `scripts/harness root` prints, not under
your cwd. Prefer the CLI: it already resolves that.
```

| line | old | new |
|------|-----|-----|
| 20 | `` check `events_<id>.jsonl` `` | `` check `scripts/harness timeline <id>` `` |
| 21 | `` quoting the file from `harness/progress/` `` | `` quoting the file from `$(scripts/harness root)/harness/progress/` `` |

### 8.5 `harness/test/harness.test.ts`

**(a) Fix the inherited-env bug.** Replace `run` (lines 105-112):

```ts
function run(script: string, args: string[] = [], cwd = REPO) {
  // A session started by the Forge daemon exports FORGE_HARNESS_ROOT, and
  // `harnessRoot()` honours it before anything else — inherited here it would
  // point every fake repo at the real checkout, so the suite would both fail
  // and write into it.
  const { FORGE_HARNESS_ROOT: _ignored, ...env } = process.env;
  const p = Bun.spawnSync(["bun", script, ...args], { cwd, env, stdout: "pipe", stderr: "pipe" });
  return {
    code: p.exitCode,
    out: new TextDecoder().decode(p.stdout),
    err: new TextDecoder().decode(p.stderr),
  };
}
```

**(b) Extend the imports** (lines 7-9):

```ts
import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
```

**(c) Add a `describe("root", …)` block** — put it immediately before
`describe("real repo", …)` (line 446):

```ts
describe("root", () => {
  test("FORGE_HARNESS_ROOT wins over the checkout", () => {
    const r = fakeRepo({ features: [] });
    const p = Bun.spawnSync(["bun", r.cli, "root"], {
      cwd: REPO,
      env: { ...process.env, FORGE_HARNESS_ROOT: "/somewhere/else" },
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(p.exitCode).toBe(0);
    expect(new TextDecoder().decode(p.stdout).trim()).toBe("/somewhere/else");
  });

  // The regression this whole change exists for: an agent standing in a
  // worktree must be told the *main* checkout, not the one under its feet.
  test("from a worktree it answers the main checkout", () => {
    const git = (cwd: string, ...args: string[]) =>
      Bun.spawnSync(["git", ...args], { cwd, stdout: "pipe", stderr: "pipe" });
    if (Bun.spawnSync(["git", "--version"]).exitCode !== 0) return; // no git, no case

    const main = realpathSync(mkdtempSync(join(tmpdir(), "forge-root-")));
    temps.push(main);
    git(main, "init", "-q", "-b", "main");
    git(main, "config", "user.email", "t@example.com");
    git(main, "config", "user.name", "t");
    mkdirSync(join(main, "harness", "src"), { recursive: true });
    mkdirSync(join(main, "harness", "progress"), { recursive: true });
    mkdirSync(join(main, "harness", "specs"), { recursive: true });
    for (const name of ["harness.ts", "cli.ts", "validate.ts", "events.ts"]) {
      copyFileSync(join(REPO, "harness", "src", name), join(main, "harness", "src", name));
    }
    writeFileSync(join(main, "harness", "features.json"), '{"project":"t","features":[]}\n');
    git(main, "add", "-A");
    git(main, "commit", "-qm", "init");

    const wt = join(main, "..", `${basename(main)}-wt`);
    git(main, "worktree", "add", "-q", "-b", "side", wt);
    temps.push(wt);

    const { FORGE_HARNESS_ROOT: _ignored, ...env } = process.env;
    const p = Bun.spawnSync(["bun", join(wt, "harness", "src", "cli.ts"), "root"], {
      cwd: wt,
      env,
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(p.exitCode).toBe(0);
    expect(realpathSync(new TextDecoder().decode(p.stdout).trim())).toBe(main);
  });
});
```

`realpathSync` on both ends matters on macOS: `mkdtempSync` returns
`/var/folders/…` and git reports `/private/var/folders/…`.

### 8.6 `.cursor/skills/` — keep the two ends from drifting

`AGENTS.md` § Harness says the Cursor variants exist. Three references, all
trivial:

- `.cursor/skills/feature/SKILL.md:26` — `harness/**` → P, leave.
- `.cursor/skills/feature/SKILL.md:54` — `Add to harness/features.json` → **W**,
  prefix `$ROOT/` and add one line pointing at `scripts/harness root`.
- `.cursor/skills/feature-go/SKILL.md:19` — `Write harness/progress/context_<id>.md`
  → **W**, prefix `$ROOT/`.

---

## 9. What proves it

### Does `harness/test/` have a suite?
Yes: `harness/test/harness.test.ts`, 34 tests in three `describe` blocks
(`validate`, `cli`, `real repo`), driven by `bun test`. It builds fake repos in
`tmpdir()` and runs `cli.ts` / `validate.ts` as subprocesses, so disk paths are
genuinely covered. It is declared in `harness/package.json` as `"test": "bun test"`.

### Does `init.sh --fast` cover this?
**No.** `--fast` runs blocks 1-3: binaries present, base files present
(checkout-local existence checks), and `bun harness/src/validate.ts`. It never
runs `bun test`, and `scripts/dev` is cargo-only — so **the harness suite is in
no gate at all today**, neither `init.sh` nor `.github/workflows/ci.yml`
(grepped: no `bun`, no `harness`).

`validate.ts` is a *detector*, not a preventer: it resolves `ROOT` correctly and
is exactly what reported feature 7's spec as missing after the fact. It cannot
stop an agent from writing to the wrong tree.

**Recommendation, flagged as a separate decision:** adding
`bun test harness/test/harness.test.ts` to `init.sh` block 3 would put the suite
behind the gate for roughly one second. It is the right call, but it widens the
blast radius of this change from "prose + one subcommand" to "the gate now fails
on a TS test". Do it as its own commit, after this one lands green, or not at
all. **Do not bundle it.**

### Verification commands

Run in order, from `/Users/kaonashi/dev/forge-node`:

```bash
# 1. The new subcommand exists and prints an absolute path.
scripts/harness root
scripts/harness --help | grep -q '^  root ' && echo "usage ok"

# 2. It survives the daemon's override.
FORGE_HARNESS_ROOT=/tmp/elsewhere scripts/harness root      # → /tmp/elsewhere

# 3. The regression case: from a real worktree it answers the main checkout.
(cd "/Users/kaonashi/Library/Application Support/Forge/worktrees/01a02a70-b898-7eb0-91a8-42253ac1ec02/test3" \
  && bun harness/src/cli.ts root)
# expect: /Users/kaonashi/dev/forge-node

# 4. doctor now names the root unconditionally.
scripts/harness doctor 7 | head -3

# 5. The suite — the baseline AND the case that was broken.
bun test harness/test/harness.test.ts
FORGE_HARNESS_ROOT=/Users/kaonashi/dev/forge-node bun test harness/test/harness.test.ts
# both must be 100% green; before the fix the second is 9 pass / 25 fail

# 6. Nothing leaked into the real state (step 5 used to write events_9.jsonl).
git status --porcelain harness/

# 7. The state is still coherent and the gate is still green.
bun harness/src/validate.ts
./init.sh --fast
./init.sh          # full — only before closing a feature; minutes
```

Steps 1-4 exercise §8.1-8.2. Step 3 is the one that would have caught the
original defect. Step 5's second invocation is the `harness/test/` fix from
§8.5(a). Step 6 guards the destructive failure mode. No Rust changes, so
`cargo` is untouched — but `./init.sh` (step 7) is the harness's own gate and
should still be run once before this is called done.

**Manual check for the prose (§8.4), which no test can assert:** re-read each of
the seven files and confirm that every row marked `W` or `R` in §2 now carries
`$ROOT/`, and that no row marked `C` does. A `C` row that grew a `$ROOT/` prefix
is a new bug: `./init.sh` and `scripts/dev check` must gate *your* checkout.

```bash
grep -n 'harness/' .claude/agents/*.md .claude/skills/*/SKILL.md | grep -v '\$ROOT' | grep -v 'scripts/harness'
```

Every surviving line must be one of the `P` rows listed in §2.

---

## 10. Deliberately left alone

| thing | why |
|-------|-----|
| `harness/features.json` tracked in git | Deliberate (§6). Gitignoring is not per-worktree, has no effect on a tracked file, and untracking breaks `init.sh` block 2 and CHECKPOINTS C1 on a fresh clone. |
| The five feature-7 files already copied into main | Instructed. Left exactly as found. |
| Feature 5's spec + gate in worktree `test` | Superseded: the human resolved its gate with `revise`, which reset the status to `pending`. Recovering it would restore a document that was already sent back. |
| Feature 7's `explore_7_themes.md` | Still orphaned in `test3`. Recovery command given in §7; it is the human's call, and it is not something `validate` checks. |
| The duplicate shadow copies in `test`, `test1`, `test3` and their `M harness/features.json` | Evidence. Deleting them destroys the proof that the bug is 3-for-3, and they are untracked/uncommitted in trees nobody is building from. Clean them after the fix lands, if at all. |
| `crates/harness-service/`, `crates/daemon/src/core.rs`, `crates/daemon/src/jobs.rs` | Already correct — they take `project_root` explicitly and never consult cwd. |
| `crates/daemon/tests/scenario_harness_cycle.rs:261` | `${FORGE_HARNESS_ROOT:-$(pwd)}` is a *fixture* asserting the daemon's export, not a template for prose. |
| `init.sh` and `.claude/hooks/fmt-and-check.sh` | Checkout-local by design; that is what they mean. |
| `rel()` in `cli.ts` and the relative paths in `resumeHint` / `nextAction` | Human display, asserted by several existing tests. `doctor` now prints `root:` unconditionally, which is where a machine should look. Changing `rel()` would churn the suite for no gain. |
| `.claude/settings.json` permissions | Unchanged. `Bash(scripts/harness*)` already matches `scripts/harness root`; §4 explains why the resolve-then-paste idiom keeps it that way. |
| Adding `bun test` to `init.sh` block 3 | Right idea, wrong commit (§9). Its own change, after this one is green. |
| The `validate` warning for a divergent shadow `features.json` | Offered in §6 as optional. Not required to close this defect. |
| `apps/tauri/` | Two other agents are working there. Nothing here touches it. |
