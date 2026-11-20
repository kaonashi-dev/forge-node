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
   `read_only: true`, titled **Forge Review** (a swarm launches one per
   hypothesis, below) — with the pack path, then wait
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
it was cancelled; any other start cancels it and begins anew. `[review]
max_swarms` (2) review workers may be alive daemon-wide, a superseded one included until it notices its
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

| Tier | Files | Diff bytes | `<graph>` | `<kb>` | `<cross-repo>` | `<runtime>` |
|------|-------|-----------|-----------|--------|----------------|-------------|
| `base` | 40 | 512 KiB | 32 KiB | — | — | — |
| `plus` | 60 | 1 MiB | 64 KiB | 32 KiB | 16 KiB | — |
| `apex` | 100 | 2 MiB | 128 KiB | 32 KiB | 16 KiB | 32 KiB |

`auto`, the default tier, is chosen once the diff is known: over 1 MiB of patch
or 50 files is `apex`; over 256 KiB, or a changed path naming `auth`,
`security`, `crypto`, `secret`, `token`, `password`, `permission`, `perf`,
`bench` or `cache`, is `plus`; anything else is `base`. A run's tier, then its
preset's, then `default_tier` win over `auto`, and the run stores the concrete
tier. Each block after the diff needs its own switch as well as its tier, is
escaped and redacted like the diff, and loses whole sections past its budget
with a coverage note. With none of them, the pack and the prompt are v1's.

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
default_tier = "auto"           # auto | base | plus | apex
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
[graph]
enabled = true                  # the codebase graph and its <graph> block
cross_repo = false              # needs [review] cross_repo_enabled too
repos = ["billing", "../shared"] # registered Forge projects: a name, or a path to a project's root
[kb]
enabled = true                  # the knowledge base in plus and apex packs
[security]
sandbox_network = false         # TREX targets get no network
sandbox_env_whitelist = ["PATH", "HOME"]
[review.auto_approve]
enabled = false
risk_ceiling = "low"            # low (no findings) | medium (P2) | high (P1) | critical (anything but P0)
min_score = 5
require_clean_diff = true       # no file left out by a tier cap
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

## Forge Review v2

Everything below runs locally and posts nothing anywhere. The report is
structured (findings, diagrams, `auto_approvable`, sandbox results) so a
publisher can be added later; none exists.

### Codebase graph

`crates/codebase-graph` holds a project's graph: `File`, `Function`, `Class`,
`Variable` (module-level constants and statics) and `ExternalCall` (one node per
name nothing in the project defines), joined by `Defines` (file → top-level
item), `Declares` (class → member), `Uses` (function → a same-file variable it
names), `Calls`, `Imports` (file → file whose stem the import's last segment, or
the one before it, names) and `Inherits`. A call resolves in its own file, then
in the files that file imports, then to a project-wide unique name. The symbols
come from `syntax-index`'s tags queries (rust, typescript, tsx, javascript,
python, go); a call inside a Rust macro's arguments is a token tree to
tree-sitter and is not seen. One graph per project, shared by its worktrees,
stored in `codebase_graph_nodes` / `codebase_graph_edges` and mirrored in
`<checkout>/.forge/review/graph/cache.bin` (MessagePack, `0600`) beside a
`.gitignore` of `*`, so the checkout stays clean.

Before a review packs, the worker brings the graph up to date: a first build
lists `git ls-files --cached --others --exclude-standard` minus the pack's
exclusions and `[ignore] paths`, and parses at most 20 000 files and 64 MiB
(the rest is `skipped` in the stats, never silently dropped). Later builds
re-parse only `git diff --name-only <graph oid> HEAD`, `git status` and what was
dirty last time, and an edge from an untouched file into a re-parsed one follows
its target by kind and name. Names, paths and signatures are redacted before
they are stored. One build per project at a time, and `.forge/review/graph/index.lock`
(`flock`) serializes daemons on one checkout. Nothing of this holds the core lock.

`<graph-N>` lists, for each function a new-side hunk touches, up to 5 callers,
5 callees and 3 similar functions (same-kind functions whose callee sets have a
Jaccard index of at least 0.5), one `path:line kind name` line each. The prompt
names it and `forgectl review graph query`.

### Swarm and `forge.review/v2`

`[review] swarm_default` (3, clamped to 1..=8) agents review each run, each an
ordinary read-only `SessionKind::Agent` titled `Forge Review · <Hypothesis>`
with its own `--session-id`, all launched together and each given the full
`timeout_secs`. A run's `swarm.size` or `swarm.hypotheses` override it; size 1
with no hypotheses is the single v1 reviewer, prompt and answer unchanged.
Hypotheses are taken in this order: `AuthBypass`, `BrokenContract`,
`ErrorSwallow`, `RaceCondition`, `NPlusOneQuery`, `MissingValidation`, then
`RetryNesting`, `DefaultMismatch`, `ResourceLeak`, `DeadCode`,
`InconsistentPattern`, `CrossLayerMismatch`, `MissingTest`, `MigrationGap`,
`ConfigDrift`. The pack is written once; each agent also reads
`slice-<Hypothesis>.md` beside it, naming the files whose path or patch matches
its keywords (every file when none does) and its instruction.

Each agent's answer is validated on its own, with the v1 rejection rule: a
rejected, silent or timed-out agent is a `hypothesis_uncovered` coverage note,
and an agent with nothing a `hypothesis discarded` note. The union is
deduplicated by fingerprint (the more severe wins, then the one with a rule),
`max_findings` applies once, and each kept finding carries its `hypothesis`. A
run fails only when every agent failed. Cancel stops every agent.
`[review] max_swarms` (2) bounds the review workers alive daemon-wide, a
superseded one included. The run's `session_id` names the first agent and
`agent_sessions` every one, so after a restart each agent is still read-only.

`parse_output` accepts `forge.review/v1` and `forge.review/v2`. v2 adds
`findings[].hypothesis` (the agent's own is kept), `findings[].kb_ref`,
`patterns` (at most 10, 200 bytes each) and `coverage_notes`; `pr_confidence`
and `diagrams` are accepted and ignored — swarm prompts ask for neither. A run
records `swarm_size` and `schema_version`.

### Diagrams

`crates/diagram-gen` draws at most two diagrams per run from the reviewed diff
and the graph, deterministically and without a model, preferring sequence, ER,
class, then flow: a **sequence** when a changed function calls, or is called
from, a function in another file (participants are files, messages the calls);
an **ER** for `.sql`, `.prisma` or migration paths (`CREATE TABLE` / `model`
blocks, columns as fields, `REFERENCES` and `<table>_id` as relations) or a
changed struct named `*Row|Record|Model|Entity|Table|Schema`; a **class**
diagram for a changed type with `Inherits` edges; a **flow** for a changed
function with more than three branch tokens (`if`, `elif`, `case`, a Rust
`=>`, `?`). A docs-only or whitespace-only diff draws nothing, a tests-only diff
at most one flow, and a file without a parser only a flow from its hunk.

The format is Forge's own (`domain::Diagram`):

```json
{"kind": "sequence", "title": "Calls across modules",
 "nodes": [{"id": "main.rs", "label": "main.rs"}, {"id": "lib.rs", "label": "lib.rs"}],
 "edges": [{"from": "main.rs", "to": "lib.rs", "label": "total"}]}
```

A sequence's nodes are its participants in order and its edges its messages in
order; an entity's label is its name, then one field per line.
`Diagram::to_mermaid` exports it. `GetReviewDiagram { review_id, kind }`
draws one kind again for exactly the range the run reviewed, from the current
graph, and stores nothing. A v1 summary's mermaid stays in the summary.

### Score and auto-approve

The score and verdict are v1's. `auto_approvable` is true only with
`[review.auto_approve] enabled`, a score at or above `min_score`, no P0, no P1
while a finding could not be verified, no file left out by a tier cap when
`require_clean_diff`, and the most severe finding within `risk_ceiling`.
Dismissing a finding recomputes it. Forge never acts on it.

### Knowledge base

One per project, in SQLite (`kb_entries`), never in the checkout. Four files:
`architecture.md` (modules to depth two with file counts, languages and
module imports), regenerated by the daemon after each graph build that moved
`git_oid`; `patterns.json`, a completed run's `patterns` when its score is at
least 4, deduplicated and kept 90 days; and `learned-rules.md` and
`false-positives.json`, derived at read time from the project's suppressions
and 👎 rows. A person may write `architecture.md` and `patterns.json` (a JSON
list of strings); the daemon never overwrites a person's `architecture.md`.
Caps: 256 KiB per file, 1 MiB per project. Everything is redacted before it is
stored. Daemon entries older than 90 days and expired patterns go with each
review's prune. `<kb-N>` (plus and apex, `[kb] enabled` and `[review]
kb_enabled`) carries the architecture sections of the diff's modules and the
learned rules; a finding cites one as `kb_ref: "architecture.md#<section>"`.

### Cross-repo context

With `[graph] cross_repo` and `[review] cross_repo_enabled`, each `repos` entry
names a registered Forge project, by name or by a path that canonicalizes to its
root; anything else is a `cross-repo repo 'X' not found` note. Each listed
project's graph is built like the reviewed one's and rebuilt when this daemon
has not refreshed it in 24 hours (a failed rebuild uses the stored graph with a
note). `<cross-repo-N>` (plus and apex) lists `<project> path name signature`
for calls from the changed functions that resolve only there: signatures,
never bodies.

### TREX

`.forge/review/sandbox.toml`, read from the user's checkout (16 KiB), never from
a pull request:

```toml
timeout_secs = 600            # per target, at most 3600
[env]                         # fixed values, never inherited; a FORGE_* name is dropped
RUST_BACKTRACE = "1"
[[targets]]
name = "test"
command = "cargo test --workspace"
```

Each target runs as `/usr/bin/sandbox-exec -p <profile> /bin/sh -c <command>`
in its own process group, killed whole at its timeout and reaped within a
bound. The profile allows everything else and denies the network (unless
`[security] sandbox_network`), writes outside the run directory, the cache
(`<checkout>/.forge/review/sandbox/cache/`, beside a `.gitignore` of `*`) and
`$TMPDIR`, reading data under Forge's home and socket directory, except the
run directory itself, and connecting to Forge's socket even with the network
on. The environment is cleared, then holds the
`sandbox_env_whitelist` names from the daemon's resolved environment, `[env]`
and `FORGE_SANDBOX_CACHE`; a `FORGE_*` name from either is dropped. The last 8 KiB of stdout and stderr is kept,
redacted. Elsewhere than macOS a run says "TREX unsupported on this platform".

An `apex` run with `[review] trex_enabled` runs every target before its agents
launch: a workspace run in the checkout, a pull request in a detached worktree
of its head under `<forge home>/review-sandbox/<run>`, removed with the run and
swept at startup. `<runtime-N>` gives each target's exit and a failing target's
output; the `sandbox_runs` row is linked to the run (`sandbox_run_id`).
`RunSandbox` runs the targets by hand, one per workspace at a time.

### Fix loop

`StartFixLoop { review_id, max_iterations }` (3 by default, at most 10) sends a
workspace review's standing findings to a new writable agent with the same
8 KiB prompt "Fix with a new agent" builds, waits for it to idle or exit within
`timeout_secs`, closes it, runs the same review (target, tier, preset,
provider, swarm size) and repeats until a review finds nothing or
`max_iterations` reviews ran. It never commits or pushes. Progress arrives as
`FixLoopUpdated`; `CancelFixLoop` stops it. A pull-request review is refused,
and a second loop on the workspace is `Conflict`. A person starts it: the GUI's
**Fix Loop** opens a confirmation first, `forgectl review fix-loop` asks on a
terminal or needs `--yes`, and no MCP tool or `review_start` flag starts one.

### Stats

`GetReviewStats` (`forgectl review stats`) gives P50/P95 wall-clock latency per
tier over completed runs, the graph hit rate (queries answered from a stored
graph against rebuilds, since the daemon started), swarm coverage (hypotheses
with a kept finding over swarm agents), the knowledge-base hit rate (runs with
a finding citing it), the TREX failure rate over sandbox-linked runs, and the
knowledge base's size.

## Daemon configuration

`[review]` in `config.toml` chooses the reviewer and its budget:

| Key | Default | Effect |
|-----|---------|--------|
| `provider` | `claude` | must declare structured review; only Claude Code does today |
| `timeout_secs` | `900` | a run with no answer by then fails and its session is stopped; capped at 24 h |
| `model`, `effort` | unset | the provider's own default; a run may override both |
| `instructions` | empty | added before every repository's `prompt.md`; over 4 096 bytes is ignored with a note |
| `swarm_default` | `3` | agents per review, clamped to 1..=8; `1` is the single v1 reviewer |
| `max_swarms` | `2` | review workers alive daemon-wide |
| `kb_enabled` | `true` | the knowledge base: refreshed, learned and packed |
| `cross_repo_enabled` | `false` | other projects' signatures, when the repository lists them |
| `trex_enabled` | `false` | run `sandbox.toml`'s targets before an `apex` review |

A reviewer is launched in the provider's **read-only** mode *and* with the
structured-review flag: Claude gets `--session-id` with a UUID Forge minted (so
the transcript is findable) and `--add-dir` for the pack. A provider without that
capability is refused, never downgraded to one that can write —
[agents.md](./agents.md). Every key with its default:
[config.example.toml](./config.example.toml).

## Clients

- `forgectl review start [--pr <n>] [--base <branch>] [--swarm-size N]
  [--hypothesis H]… --wait`, `show [--diagram <kind> [--format forge|mermaid]]`,
  `status`, `cancel` (the MCP `review_start` tool takes `pr`, `swarm_size` and
  `hypotheses`); `rules check` prints the repository's presets and rules (exit 1
  on an invalid file); `bench [--swarm-size N]` replays a corpus of
  reintroduced bugs. `graph rebuild [--full]`, `graph query --callers|--callees|
  --similar <fn>` (`--path <a> <b>`, `--imports <file>`, `--external-calls
  <file>`; with no daemon, from `cache.bin` when its oid is `HEAD`'s) and `graph
  show [--format dot|mermaid]` (MCP `review_graph_query`); `kb show [--file f]`,
  `kb edit <file>`, `kb export [--output path]` (MCP `review_kb_get`);
  `sandbox run [--target name]` (exit 1 when a target failed) and `sandbox
  results <id>` (MCP `review_sandbox_run`); `fix-loop [--review id]
  [--max-iters N] [--yes]`; `stats`.
- The Review mode shows a score badge above the summary, a **Diagrams** tab
  (each diagram drawn as SVG from theme colours), a **Graph** tab (each
  reviewed file's imports and external calls), a hypothesis chip on swarm
  findings, a collapsible knowledge-base section with the cited entries, the
  TREX targets of an `apex` run, and **Fix Loop** beside Fix with a new agent.
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
