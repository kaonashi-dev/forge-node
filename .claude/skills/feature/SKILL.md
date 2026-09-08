---
name: feature
description: Registers a specification as a harness feature and triggers research + spec writing. Stops at the human gate, without touching code.
argument-hint: "\"<task specification>\""
disable-model-invocation: true
allowed-tools: Agent, Read, Grep, Glob, Write, Edit, Bash(${CLAUDE_PROJECT_DIR}/init.sh*), Bash(${CLAUDE_PROJECT_DIR}/scripts/harness*), Bash(bun harness/src/validate.ts), Bash(git status*), Bash(git diff*), Bash(git rev-parse*)
---

# Role: harness lead

During this turn you act as the **lead**: you decompose and coordinate, you do
**not implement**. This role only applies to the flow that starts here; outside
of it the repository works as usual.

The specification the human gave you is:

$ARGUMENTS

## Hard rules of the role

- ❌ Do not edit anything under `crates/`. Not with Edit, not with Write, not
  with Bash.
- ❌ Do not mark any feature as `done`.
- ❌ Do not `git commit` or `git push`.
- ✅ You do edit `harness/**` (it is your state) and you do launch subagents.

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
path into every Read / Write / Edit, into every shell command, and into every
prompt you hand a subagent. Do not export it: each Bash call is a fresh shell
and the variable will not survive.

`scripts/harness`, `./init.sh` and `scripts/dev` stay **relative** on purpose —
they act on the code in *your* checkout, and `scripts/harness` resolves the
state root by itself.

Append to the event log **only** through `scripts/harness event`. Never write
`events_<id>.jsonl` by hand: the CLI is what puts the line in the right file
with a real timestamp.

## Anti-broken-telephone rule

Every subagent you launch must receive the **explicit** instruction to write its
result to a file and return you **a single line** with the path. You do not
receive diffs, reports or findings in the chat: you receive paths and you read
them from disk if you need them. Example of a correct instruction:

> "Find out how `crates/daemon/src/server.rs` routes requests and what type
> each handler returns. Write the findings in
> `$ROOT/harness/progress/explore_1_server.md`. Your answer must be only
> `done -> $ROOT/harness/progress/explore_1_server.md`."

Resolve `$ROOT` with `scripts/harness root` **before** you paste this: the
subagent receives the literal absolute path, never the placeholder. A subagent
runs with its own cwd, so a relative `harness/progress/…` in the prompt is how
research gets written into a worktree and lost.

## Protocol

### 1. Check the environment

Run `scripts/harness gate --fast` (or `./init.sh --fast`). If it fails, **stop
and report it**: nothing is orchestrated on top of a harness that does not
validate its own state. `scripts/harness status` summarises the features and the
next action.

Also check there is no feature already in `in_progress` or `in_review`. If there
is, stop: one at a time.

### 2. Register the feature

Add an entry to `$ROOT/harness/features.json`:

- `id`: the next free integer.
- `slug`: kebab-case, short, derived from the title.
- `title`: one line.
- `spec_raw`: **the human's text, verbatim**. Do not rewrite it nor summarise
  it: it is the only source of truth for what was asked and the `reviewer` uses
  it to detect scope drift.
- `crates`: the crates you expect to touch (`domain`, `protocol`, `daemon`,
  `client`, `forge-tauri`, `agents`, `persistence`, `git-service`,
  `terminal-core`…).
- `acceptance`: 3-6 objective criteria derived from `spec_raw`. If `spec_raw` is
  too vague to write them, **ask the human now** instead of inventing them.
- `status`: `"pending"`, `review_rounds`: `0`, `gate_attempts`: `0`,
  `created_at`: today's date.

Append events:

```bash
scripts/harness event <id> feature_registered --data '{"slug":"<slug>","title":"<title>"}'
```

Note the feature, the time and your plan in `$ROOT/harness/progress/current_<id>.md`.
One note per feature: another worktree of this project may have its own
feature running at the same time.

**Alternative trigger (factor 11):** if the human gave a GitHub issue instead of
free text, run `scripts/harness from-issue <number|url>` and continue from step
3 with the returned id.

### 3. Scale the effort

| Complexity | What you launch |
|------------|-----------------|
| Trivial: one file, one crate, obvious precedent | straight to the `spec-author` |
| Medium: 2-3 files within one crate | 1 scoped `Explore` → `spec-author` |
| Complex: crosses crates, touches protocol or persistence | 2-3 `Explore` **in parallel** (one scoped question each) → `spec-author` |
| Very complex | split it into separate features and apply the table again |

The `Explore` agents write to `$ROOT/harness/progress/explore_<id>_<topic>.md`
— the resolved absolute path, spelled out in their prompt. Launch
them in a single message so they run at the same time. After each explore:

```bash
scripts/harness event <id> explore_done --data '{"path":"harness/progress/explore_<id>_<topic>.md"}'
```

### 4. Launch the `spec-author`

With `subagent_type: "spec-author"`, passing it the feature id and **the paths**
of the `explore_*.md` files you generated (so it does not research the same
thing again: that is the most expensive part of the cycle).

### 5. Stop at the human gate

When the `spec-author` returns `spec_ready`, your turn ends. Report to the
human, in a few lines:

- the path of `harness/specs/<id>-<slug>/`;
- the path of `harness/progress/gate_<id>.md`;
- the `R<n>` items in a list, one line each;
- which `design.md` decision deserves their attention (the discarded
  alternative);
- that to continue they should type `/feature-go <id>`.

**Do not launch the implementer.** Spec approval is human.
