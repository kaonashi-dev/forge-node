# Forge Review

A structured review of one workspace's changes, or of a GitHub pull request's
head. It is not the agent-recipe pull-request review
([`pr_review.rs`](../apps/ui/src/pr_review.rs)): it runs in the checkout the
user is working in, sends a bounded pack to a **read-only** agent session, and
validates the answer against the diff before anything is shown.

Three pieces own it, in that order:

- `crates/review` is the pure engine — pack, rules, presets, answer parsing,
  validation and scoring. It touches no disk, process or clock.
- `crates/daemon/src/review.rs` runs one worker thread per run:
  `diff → pack → model → validate`. Git, the pack, the transcript and SQLite all
  happen outside the core lock.
- `apps/ui/src/code_review.rs` is the workbench's Review mode; `forgectl review`
  is the same requests from a shell.

## A run

1. Resolve the base: the request's `base`, else the repository's default
   branch, diffed from the merge base with the current `HEAD`. Without one, the
   run reviews the uncommitted changes only and says so in its coverage notes.
2. Build the pack (below) for the resolved tier.
3. Launch the reviewer — an ordinary `SessionKind::Agent` with
   `read_only: true`, titled **Forge Review** — with the pack path, then wait
   for its hook to report `Idle` (or its session to exit). A reviewer that
   reports `Idle` or `Waiting` without a parseable answer fails the run after a
   2-second grace, so one stuck on a permission prompt does not hold a slot. A
   run whose reviewer never answers fails at `[review] timeout_secs` (at most
   24 h). However the run ends, its reviewer session is stopped and closed.
4. Parse the last assistant message for the `forge.review/v1` JSON object, then
   validate it. The full diff, the pack and the reviewer's transcript are never
   stored; a finding keeps only its capped evidence and suggestion.

A workspace has one run in flight. A start with the same head, base, provider,
model, effort, tier, preset and instructions answers with that run's id unless
it was cancelled; any other start cancels it and begins anew. Two review workers
may be alive daemon-wide, a superseded one included until it notices its
cancel: past that, a start that would not join a running run is `Conflict`, even
one that would supersede. A cancel that lands while the answer is validated
still wins, and no findings are saved for it. After every run the workspace is
pruned: anything older than 90 days, and anything past the newest 20. A run
whose reviewer session row still exists is kept, because that row is what makes
a restart read-only; a finished run closed its reviewer, so only a run the
daemon died under keeps one.

Validation is what makes a finding a finding:

- a category outside `logic`, `security`, `performance`, `style` and `docs`
  is rewritten to `logic` before anything else reads it;
- a `rule_id` is kept only when it names an in-scope rule whose `scope`
  matches the finding's own path, and then caps the finding's severity;
  otherwise it is cleared. A finding in a disabled category without a rule is
  dropped;
- its path must be in the diff, without `..` and not absolute, and its
  `evidence` must occur in the file. The comparison runs over non-blank lines on
  both sides with whitespace collapsed, and reads the first 40 evidence lines.
  A finding that fails either check is dropped and counted as
  `Coverage::unverified`, with the note "N finding(s) could not be verified".
  A line anchor is snapped to the nearest occurrence within ±3 lines and kept
  only inside a new-side hunk; otherwise the finding becomes general;
- a finding on an excluded file (below) is kept as general with an empty
  evidence: the file never reached the pack, and its text may hold secrets;
- `confidence` below the configured `min_confidence` is dropped. Overlapping
  findings dedupe, and so does a general finding with the fingerprint of one
  already kept; the more severe one stays. The project's feedback suppressions
  drop what they match (below), and only then `max_findings` trims the tail;
- title, body, evidence, suggestion and the summary are redacted like the
  pack before they are stored or fingerprinted;
- the answer is rejected when `(invalid anchors + unverified) × 2` exceeds
  `anchor attempts + unverified`, where an attempt is a verified finding that
  named a line. General findings neither help nor hurt: the model reviewed
  something other than this change.

The summary may carry one mermaid diagram; a suggested fix is capped text, never
a patch Forge applies. A run that outlives the daemon is metadata: with
`sessions.persist_history`, its reviewer session comes back `Orphaned` and
startup still knows it is read-only, from the run's `session_id`.

## Pull requests

`ReviewTarget::PullRequest { number, base }` reviews a pull request from the
project's existing checkout; nothing moves its `HEAD` or working tree, and no
scratch worktree is made. The worker fetches from the remote whose URL names the
pull request's repository (from the pull-request cache), else from the remote
the listing reads (`origin`, or the only one), in one `run_git_network` fetch:
`refs/pull/<n>/head` into `refs/forge/review/<n>/<run>/head` and `base` (the
cached pull request's base, else the remote's `HEAD`) into
`refs/forge/review/<n>/<run>/base`, with an empty `--refmap` so the remote's
own refspec updates no remote-tracking ref. Each run has its own refs, and they
are deleted when it ends. It diffs their merge base to that head. The pack says the working directory is not the
head and that head files read with `git show <head>:<path>`; validation reads
them from the object store the same way, capped like a working-tree read. The
checkout's own `AGENTS.md` and `.forge/review/` are the instructions: the
user's, not the pull request's.

A pull-request run has its own in-flight slot, `<workspace>#pr<n>`, so it
neither joins nor supersedes the workspace's review; its join key leaves out
the head, which is unknown until the fetch. `base` is checked with
`check-ref-format --branch` before anything runs. A pull-request run is never
stale, and Send and Fix are off for it.

## Lineage

A completed run is compared with the newest completed run before it of the same
workspace and preset, and of the same branch (for a workspace run, the branch at
the run's start) or the same pull request. Each finding is stored `new` or
`persisting` by fingerprint; `GetReview` computes `resolved` — the previous
run's findings this one no longer reports — and names `previous`. A fingerprint
the reviewer reported but validation left out (suppressed, below the floor, past
`max_findings`) is kept in `Coverage::withheld`, so it is not counted as
resolved. A finding dropped as unverified is not withheld: repeated with
evidence the file no longer holds, a fixed finding shows as resolved. Nothing is
copied between runs.

## Feedback and suppression

`SetFindingFeedback` stores 👍, 👎 or a dismissal in `review_feedback`, keyed by
run and finding, with the project and the finding's fingerprint, rule, category
and severity. Rows have no foreign key to the run, so pruning a run keeps its
feedback. Dismissing (or clearing a dismissal) rescores the run without its
dismissed findings.

Suppressions are never stored; `review::suppressions` derives them from the
project's newest 10 000 rows each time:

- a rule with 3 dismissals in the last 30 days and no 👍 for it in that window;
- `style` with 5 dismissals in 30 days and no 👍;
- any dismissed fingerprint, only at the severity it was dismissed at.

P0 and `security` findings are never suppressed. What was suppressed is a
coverage note. `GetReviewConfig` lists the active suppressions;
`ClearReviewSuppression` lifts one: a fingerprint's dismissals are deleted, so
the finding comes back, and each run that held one is rescored and broadcast
as `ReviewUpdated`; a rule's or `style`'s stop counting toward it, and each
finding they dismissed stays dismissed.

## Send and Fix

**Send to agent** (one finding) and **Send all** (every finding not dismissed)
queue a `SendSessionPrompt` with `when: Queue` for the checkout's live,
foreground agent that was active last, never a reviewer. A prompt holds whole
findings within 8 KiB and says how many were left out. **Fix with a new agent**
launches a writable agent, chosen like *Resolve with AI*, with the same prompt.

## Inline findings

`GetReviewDiff` answers with exactly the range a run reviewed — merge base to
head for a pull request; for a workspace, its merge base to `tree_oid`, the
working tree written as a tree object through a throwaway index when it was
reviewed (a patch dropped for size, and a binary, are left out of it) — as a
local read that never touches the network. The tree is unreachable, so a
`git gc` past `gc.pruneExpire` can remove it; the read then says so. The Review mode's
**Diff** tab draws it with each standing, anchored finding under its new-side
line; the Changes view puts the same notes on the live working tree while the
workspace's latest run is a completed, fresh workspace review. Annotations are
rebuilt when the report or the shown files change, never per frame.

## The pack

The prompt points at a temporary `pack.md`, under `<forge home>/review-packs/<run>/`
(directories `0700`, the file `0600`, never beside a configurable worktrees
root). It is removed when the run ends, and the whole `review-packs` directory
at daemon startup. The answer is read from the reviewer's transcript. The pack
carries `<project_context-N>`, the in-scope `<rules-N>`, `<commits-N>` and
redacted `<diff-N>` blocks, where `N` is a random nonce per run, and the prompt
names the same tags. Inside data, any opening or closing tag of the four, in
any case or spacing, has its `<` escaped, and a `path` attribute escapes `&`,
quotes, angle brackets and control characters. Commit subjects are redacted
and neutralised like the diff.

| Tier | Files | Diff bytes |
|------|-------|-----------|
| `base` | 40 | 512 KiB |
| `plus` | 60 | 1 MiB |
| `apex` | 100 | 2 MiB |

Lockfiles and snapshots, `.env*`, `*.pem`/`*.key`, `.npmrc`, `.pypirc`,
`.netrc`, `*.tfvars`, `*.p12`/`*.pfx`, generated and `dist/` paths, binaries and
files already over the diff budget are left out. File names and `[ignore]
paths` match in any case. What was left out is recorded in `Coverage`, not
silently dropped, and the tier note counts only the files its cap cut.
Token-shaped secrets (AWS, GitHub, Anthropic and OpenAI `sk-`, Slack, GitLab,
Google API and npm tokens, JWTs, PEM private keys, the password in
`scheme://user:pass@`, and quoted `secret`/`token`/`password` values) are
replaced with `«redacted»` before a line reaches the pack.

## Repository files (`.forge/review/`)

The files are read from the workspace root, each inside the checkout and each
within its own byte cap. A bad file is a coverage note, never a refusal: the
review still runs with the layers it could read.

| Path | Cap | Purpose |
|------|-----|---------|
| `config.toml` | 64 KiB | the keys below |
| `prompt.md` | 16 KiB | added to every review's instructions |
| `context.md` | context budget | project background, like `prompt.md` but data rather than instructions |
| `rules/*.md` | 100 files | the project's rules |
| `presets/*.toml` | 50 files | presets that replace or extend the built-ins |

`config.toml` (all keys optional):

```toml
language = "en"
default_preset = "standard"     # standard | deep | security | custom | a repository preset
default_tier = "base"           # base | plus | apex
strictness = "normal"           # relajado | normal | estricto
max_findings = 40               # 1..=200
min_confidence = 0.5            # 0.0..=1.0
[context]
project_docs = true             # AGENTS.md and CLAUDE.md, before `include`
include = ["docs/domain.md"]
max_bytes = 65536               # 1..=256 KiB, every context file together
[categories]
logic = true
security = true
performance = true
style = false
docs = false
[ignore]
paths = ["vendor/**", "*.generated.ts"]
```

A rule is Markdown with frontmatter (the id must be kebab-case; `scope` is a
JSON list of globs):

```markdown
---
id: no-unbounded-read
title: Reads are clamped before allocating
severity: P1
category: security
scope: ["crates/**/*.rs"]
enabled: true
---
Any byte count that arrives from a file must be clamped before the allocation.
```

A preset is a partial override; a file whose id matches a built-in replaces it
in place, and an unknown rule id is ignored with a note:

```toml
name = "Review the change"
detail = "Correctness, missing tests, and anything that would block a merge."
tier = "base"
rules = ["*"]                   # or the ids it applies
instructions = "Read the caller before judging the change."
[categories]
style = false
```

The built-ins are `standard`, `deep` (apex), `security` (security only) and
`custom`, which needs instructions from the run or from a repository preset
that redefines it.

## Daemon configuration

`[review]` in `config.toml` chooses the reviewer and its budget:

| Key | Default | Effect |
|-----|---------|--------|
| `provider` | `claude` | must declare structured review; only Claude Code does today |
| `timeout_secs` | `900` | a run with no answer by then fails and its session is stopped; capped at 24 h |
| `model`, `effort` | unset | the provider's own default; a run may override both |
| `instructions` | empty | added before every repository's `prompt.md`; over 4 096 bytes is ignored with a note |

A reviewer is launched in the provider's **read-only** mode *and* with the
structured-review flag: Claude gets `--session-id` with a UUID Forge minted (so
the transcript is findable) and `--add-dir` for the pack. A provider without that
capability is refused, never downgraded to one that can write —
[agents.md](./agents.md). Every key with its default:
[config.example.toml](./config.example.toml).

## Clients

- `forgectl review start [--pr <n>] [--base <branch>] --wait`, `show`, `status`,
  `cancel` (the MCP `review_start` tool takes `pr`); `rules check`
  prints the repository's presets and rules (exit 1 on an invalid file);
  `bench` replays a corpus of reintroduced bugs.
- The GUI's **Review** mode starts a run, shows stage and progress, groups
  findings by severity or file, marks lineage and lists what was resolved, takes
  feedback, lists suppressions with Undo, sends findings to an agent, and
  cancels. A pull request's detail view starts a **Forge Review** of it. A
  checkout's workspace review and each pull request's keep their own lane in
  the tab; one landing never replaces another, and a pull-request finding's
  location opens the reviewed diff, not the checkout's file.
- Wire requests and the `ReviewUpdated` event:
  [protocol.md](./protocol.md). The stored run/findings rows:
  [persistence.md](./persistence.md).
