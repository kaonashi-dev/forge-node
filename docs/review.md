# Forge Review

A structured review of one workspace's changes. It is not the GitHub pull-request
review ([`pr_review.rs`](../apps/ui/src/pr_review.rs)): it runs in the checkout the
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
   for its hook to report `Idle` (or its session to exit). A run whose reviewer
   never answers fails at `[review] timeout_secs` and the session is stopped.
4. Parse the last assistant message for the `forge.review/v1` JSON object, then
   validate it. The full diff, the pack and the reviewer's transcript are never
   stored; a finding keeps only its capped evidence and suggestion.

A workspace has one run in flight. A start with the same head, base, provider,
model, effort, tier, preset and instructions answers with that run's id unless
it was cancelled; any other start cancels it and begins anew. Two runs may be in
flight daemon-wide (`Conflict` past that). After every run the workspace is
pruned: anything older than 90 days, and anything past the newest 20. A run
whose reviewer session row still exists is kept, because that row is what makes
a restart read-only.

Validation is what makes a finding a finding:

- its `evidence` must occur in the file, and a line anchor is snapped to the
  nearest occurrence within ±3 lines and kept only inside a new-side hunk;
  otherwise the finding becomes general;
- an unknown `rule_id` is cleared, a known one caps the finding's severity, and a
  finding in a disabled category without a rule is dropped;
- `confidence` below the configured `min_confidence` is dropped, overlapping
  findings dedupe, `max_findings` trims the tail;
- if more than half the claimed anchors cannot be confirmed inside the diff,
  the whole answer is rejected — the model reviewed something other than this
  change.

The summary may carry one mermaid diagram; a suggested fix is capped text, never
a patch Forge applies. A run that outlives the daemon is metadata: with
`sessions.persist_history`, its reviewer session comes back `Orphaned` and
startup still knows it is read-only, from the run's `session_id`.

## The pack

The prompt points at a temporary `pack.md`; the answer is read from the reviewer's
transcript. The pack carries `<project_context>`, the in-scope `<rules>`, the
commits and redacted diffs, each in a block whose closing tag is neutralised
inside data so a diff cannot end the block early.

| Tier | Files | Diff bytes |
|------|-------|-----------|
| `base` | 40 | 512 KiB |
| `plus` | 60 | 1 MiB |
| `apex` | 100 | 2 MiB |

Lockfiles and snapshots, `.env*`, `*.pem`/`*.key`, generated and `dist/` paths,
binaries and files already over the diff budget are left out; what was left out
is recorded in `Coverage`, not silently dropped. Token-shaped secrets are
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
| `timeout_secs` | `900` | a run with no answer by then fails and its session is stopped |
| `model`, `effort` | unset | the provider's own default; a run may override both |
| `instructions` | empty | added before every repository's `prompt.md`; over 4 096 bytes is ignored with a note |

A reviewer is launched in the provider's **read-only** mode *and* with the
structured-review flag: Claude gets `--session-id` with a UUID Forge minted (so
the transcript is findable) and `--add-dir` for the pack. A provider without that
capability is refused, never downgraded to one that can write —
[agents.md](./agents.md). Every key with its default:
[config.example.toml](./config.example.toml).

## Clients

- `forgectl review start --wait`, `show`, `status`, `cancel`; `rules check`
  prints the repository's presets and rules (exit 1 on an invalid file);
  `bench` replays a corpus of reintroduced bugs.
- The GUI's **Review** mode starts a run, shows stage and progress, groups
  findings by severity or file, and cancels.
- Wire requests and the `ReviewUpdated` event:
  [protocol.md](./protocol.md). The stored run/findings rows:
  [persistence.md](./persistence.md).
