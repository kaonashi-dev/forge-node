# Agent providers

An **agent is a terminal session running a known CLI** (ADR-006). Forge does
not speak to agents over an API; it spawns their interactive TUI in a PTY and
renders it like any shell. Everything provider-specific lives in
`crates/agents` (principle P2) — no other crate branches on a provider id.

Plan references: §7.5, §7.6, §13. ADR-007 (declarative provider registry).

## Built-in providers (`builtins.rs`)

| id | Display name | Binary candidates (in order) | Probe | Marker | Profile fields | Resume | Prompt | Read-only |
|----|--------------|------------------------------|-------|--------|----------------|--------|--------|-----------|
| `claude` | Claude Code | `claude` | `--version`, 3 s | — | `CLAUDE_CONFIG_DIR` (directory), `--model`, `--agent` | `--resume <id>` | positional | `--permission-mode plan` |
| `codex` | Codex CLI | `codex` | `--version`, 3 s | — | `CODEX_HOME` (directory), `--model`, `--profile` | `resume <id>` (subcommand) | positional | `-s read-only` |
| `opencode` | OpenCode | `opencode`, `opencode2` | `--version`, 3 s | — | `OPENCODE_CONFIG_DIR` (directory), `XDG_DATA_HOME` (directory), `--model`, `--agent` | `--session <id>` | `--prompt <text>` | `--agent plan` |
| `cursor` | Cursor CLI | `agent`, `cursor-agent` | `--version`, 3 s | output or unambiguous basename must contain `cursor` | — | — | positional | `--mode ask` |

All four are interactive TUIs and take no `default_args`. Each capability flag
restates whether the matching spelling is declared — `supports_resume` a
`ResumeStyle`, `supports_initial_prompt` a `PromptStyle`, `supports_review` a
`ReviewStyle` — so the GUI can read the flag and the launch builder the
spelling. OpenCode is the one that names its prompt: `opencode [project]`
reads its positional as a *directory*, so a prompt put there would launch it
in a folder named after the prompt.

## Read-only mode (§16.9)

`AgentDescriptor.review` is how a provider is launched so that it reads and
reasons but does not write — the third sibling of `ResumeStyle` and
`PromptStyle`, and declared for the same reason. It carries the flags and the
name the provider itself uses for the mode, which is what lets a menu say which
of four different postures the user is picking.

It exists because an automatic pull-request review starts in a checkout the
user is working in. `build_launch` appends those flags **after** a profile's own
arguments so they win — a profile naming OpenCode's `build` agent must not turn
a review into a session that can edit the checkout — and refuses with
`AgentError::ReviewUnsupported` when a read-only launch is asked of a provider
that declares none. Refused, never downgraded: a review that can write is not a
weaker review, it is a different thing.

The daemon remembers which sessions were launched this way (`Inner::read_only`,
runtime-only like `resumed_from`) and re-applies the flags on a restart. That is
the opposite of what it does with the initial prompt, which is deliberately not
re-sent: a restarted review that came back able to write would be a different
session wearing the same name.

## Resume (§13.5)

`AgentDescriptor.resume` is how a provider re-enters one of its own earlier
sessions, as data rather than behavior: `ResumeStyle::Flag` for the two that
spell it as a flag, `ResumeStyle::Subcommand` for Codex, whose `resume` has to
lead the command line. `build_launch` puts those arguments after
`default_args` and *before* a profile's own — a subcommand after a flag would
not parse — and refuses with `AgentError::ResumeUnsupported` when a resume is
asked of a provider that declares none, rather than silently starting a fresh
conversation.

Cursor declares no spelling on purpose: its `--resume` takes an *optional* chat
id, so a following argument is ambiguous to its parser, and nothing discovers
Cursor history to hand it an id (`external_agents` reads Claude and opencode).

The id is the provider's own — the one its transcript records — and every CLI
resolves it relative to its working directory, so a resumed session is launched
in the directory the original run happened in. See `docs/ui.md` for the history
panel that sends them.

The `cursor` marker exists because `agent` is an ambiguous name — on many
machines it is the Grok CLI. That generic candidate is `Rejected` when its
probe output lacks the marker, then detection falls through to `cursor-agent`.
The unambiguous `cursor-agent` basename is accepted even if its version output
is only a number.

## Two levels of adapter (ADR-007)

1. **Descriptor** (`AgentDescriptor`) — static data: candidates, probe,
   capabilities. This is what every MVP built-in is.
2. **`AgentAdapter` trait** — for providers that need behavior (initial
   prompts, session detection). The MVP ships **no** real adapter; the generic
   `DescriptorAdapter` wraps a descriptor with no special behavior. Resume did
   not need one: its two spellings fit in the descriptor as data.

### Adding a provider

1. Add one `descriptor(id, display, &[candidates], expect)` line to
   `builtins()` in `crates/agents/src/builtins.rs`, keeping picker order. Give
   it an `expect_substring` whenever a candidate name is generic enough to
   belong to something else.
2. Nothing else changes: `AgentRegistry::new()` seeds itself from `builtins()`,
   the daemon exposes it through `ListAgentProviders`/`GetSnapshot`, and no
   other crate branches on provider id (P2).
3. Cover it in `crates/agents/src/detection.rs` tests with a fake executable on
   a temp PATH (`test_support::write_script`).

A provider that needs *behavior* (an initial prompt, a resume that is not just
"these tokens, then the id") implements `AgentAdapter` instead — but
`AgentRegistry` has no public registration API today, so wiring one in also
means extending the registry.

Custom user-defined *providers* from `config.toml` (§13.4) are post-MVP and not
implemented; the owned `String` fields in `AgentDescriptor` already allow
descriptors to travel over IPC when they arrive. Launch **profiles** (below)
are implemented and orthogonal: they key on `provider_id`, so a future custom
provider gains them for free.

## Launch profiles (§13.4)

A **profile** is a named way to start a known provider — its own command,
arguments and environment. It replaces the hand-written shell wrapper people
write to run two accounts:

```fish
function claude-work
    set -lx CLAUDE_CONFIG_DIR "$HOME/.claude-work"
    mkdir -p "$CLAUDE_CONFIG_DIR"
    command claude $argv
end
```

A profile is *not* a provider: it borrows the descriptor's icon, detection,
binary candidates and usage source, and overrides only how the process starts.
`domain::AgentProfile` carries `{ provider_id, name, executable, args, env }`;
the bare provider is the implicit profile (`profile_id: None`).

**Provider-specific knowledge stays here** (P2): each descriptor declares the
fields its profile editor should offer, as data.

```rust
ProfileField {
    label: "Config directory",
    help:  "A separate account: its own login, settings, history and plugins.",
    effect: ProfileFieldEffect::Env { name: "CLAUDE_CONFIG_DIR", is_directory: true },
}
```

`is_directory` is what makes the daemon run the wrapper's `mkdir -p`:
`ensure_profile_dirs(descriptor, env)` creates those directories with `0700`
before the launch. Nothing matches on a provider id to do it.

`CLAUDE_CONFIG_DIR` is the documented account switch for Claude Code (settings,
session history and plugins move with it); `CODEX_HOME` is its Codex
equivalent, and `usage/codex.rs` already reads credentials through it.

OpenCode is the one that needs two directories, because it splits what the
other two keep together: `OPENCODE_CONFIG_DIR` moves the directory
`opencode.json` is read from — and with it the agents, commands and plugins
declared beside it — while credentials and session storage stay where they
were. The account switch is `XDG_DATA_HOME`, under which OpenCode keeps
`opencode/auth.json` and `opencode/storage`. It is a standard XDG variable
rather than an OpenCode one, so the field's help says as much: every process
OpenCode itself starts inherits it. Its two flags are `--model`, which takes a
`provider/model` pair rather than a bare alias, and `--agent`, which picks the
configured agent the TUI starts on.

Because the suggested path is derived from the provider id rather than from the
variable name (`settings::directory_base`), a generic variable still reads as
the provider's: `XDG_DATA_HOME` suggests `~/.opencode-data-work`, never the
same stem as `~/.opencode-work`.

Two consequences of the account switch are worth stating. A profile that moves
`XDG_DATA_HOME` moves the transcripts the history panel scans, and discovery
reads the daemon's own environment (`external_agents::opencode_storage`), so
those sessions do not appear there — the same gap Claude profiles have with
`CLAUDE_CONFIG_DIR`. And Cursor declares no fields at all: it documents no
configuration switch, so there is nothing honest to suggest; a profile for it
is still writable by hand.

### Rules

- A profile belongs to one provider. Launching Codex with a Claude profile is
  `InvalidRequest`, not a silently ignored field.
- Names are unique per provider, case-insensitively (a unique index in SQLite,
  surfaced as `Conflict`): two profiles cannot render as the same menu entry.
- A profile may not set `TERM`, `TERMINFO`, `COLORTERM`, `FORGE_SESSION_ID` or
  `FORGE_WORKSPACE` (`domain::RESERVED_PROFILE_VARS`) — Forge owns the terminal
  contract with the child. The builder drops such a variable with a warning;
  the daemon refuses to store one at all.
- A profile with its own `executable` is version-probed when it is **saved**,
  exactly like a `SetProviderExecutable` override, so the daemon still never
  launches a binary no probe accepted. At launch time only its presence and
  executable bit are re-checked.
- Deleting a profile never touches sessions it already started; `RestartSession`
  re-applies the profile, and refuses if it is gone rather than silently
  restarting under a different account.
- `env` values are stored in clear text like every other row (ADR-009). The
  editor says so and steers users towards a config directory rather than an
  API key.

## Detection (`detection.rs`, §13.1)

Runs in the daemon at startup (in the background, off the snapshot path) and on
`RefreshAgentDetection`, always against the **resolved login-shell
environment** (see [terminal.md](./terminal.md#login-shell-environment-12-daemonsrcenvironmentrs)).

```
override set?  ──yes──►  probe that path  ──►  Installed | Rejected | ProbeTimeout
      │ no
      ▼
for candidate in binary_candidates:
    find executable on env.path_entries       (none? → next candidate)
    run `candidate --version` under timeout   (stdout+stderr drained on threads)
    ├─ marker present, basename unambiguous, or none required
    │                                  → Installed { executable, version }   (stop)
    ├─ marker missing from ambiguous candidate
    │                                  → Rejected { candidate, reason }      (next)
    └─ timeout                         → ProbeTimeout                        (stop)
no candidate left → the last Rejected, else NotFound
```

Notes:

- A user override (`SetProviderExecutable`) skips the PATH search but is still
  verified by the probe.
- When every candidate was rejected, the *last* rejection is reported rather
  than `NotFound` — the picker can then say why (`agent` was the Grok CLI)
  instead of "not installed".
- A candidate that cannot even be spawned is also `Rejected` (reason: "failed
  to run version probe: …"); only a hang produces `ProbeTimeout`.
- `version` is the first non-empty line of stdout **and** stderr combined
  (some CLIs print their version on stderr).
- The probe child is killed and reaped on timeout; a hung `--version` can never
  block the daemon.
- `DetectionResult.checked_at` is stamped when detection finishes; results are
  cached by the daemon in `Inner.detections` (`core.rs`) — *not* in
  `AgentRegistry`, whose own detection map stays empty in production — and
  broadcast as `AgentDetectionChanged`.
- `RefreshAgentDetection { provider_id }` re-probes exactly one provider when
  `provider_id` is `Some` (leaving the other cached results untouched) and all
  of them when it is `None`. `SetProviderExecutable` re-detects **all**
  providers after storing the override.

## Registry (`registry.rs`)

`AgentRegistry` holds the built-in descriptors (each in a `DescriptorAdapter`),
per-provider executable overrides, and the latest `DetectionResult`s. It is
synchronous by design; the daemon side is thin orchestration living directly in
`daemon/src/core.rs` (there is no separate `AgentService` module) and it decides
where detection runs — a background thread at startup, the caller's thread on
refresh.

Overrides survive a restart: `SetProviderExecutable` writes to both the
registry and the `provider_overrides` table, and `Daemon::start` replays that
table into the registry (§13.2). Both detection paths additionally read the
override straight from SQLite, so the persisted value wins regardless of
registry state.

## Launch (`descriptor.rs`, §13.3)

`build_launch(descriptor, req, env) -> SpawnSpec`:

- `program` — absolute path: the request's `executable_override`, else the
  first candidate found on `env.path_entries`. `AgentError::NotInstalled`
  otherwise (surfaced as `ErrorCode::ProviderNotInstalled`).
- `args` — `descriptor.default_args` followed by `req.extra_args` (the
  profile's arguments, or empty).
- `env` — the complete resolved environment plus `TERM=xterm-256color` and
  `COLORTERM=truecolor`.

`build_launch_with_env_overlay(descriptor, req, env, overlay)` is the same
builder plus a profile's environment, applied *after* the terminal hints and
skipping any reserved variable. `build_launch` is that function with an empty
overlay, so there is one code path.

`FORGE_SESSION_ID` and `FORGE_WORKSPACE` are deliberately **not** added here:
the crate has no `SessionId`. The daemon injects them when it spawns the PTY
(`core.rs`), keeping this builder pure.

## Client-facing view

`ListAgentProviders` and the `providers` field of `GetSnapshot` return
`ProviderInfo { descriptor, detection }` for every provider, in picker order;
`GetSnapshot` also carries `agent_profiles`, and every change to that set is
broadcast whole as `AgentProfilesChanged` (like usage and detection).
The GUI shows installed providers as launchable and the others with their
`DetectionStatus` (`NotFound`, `Rejected { reason }`, `ProbeTimeout`) and an
"override executable" affordance.

## Idle agents (`crates/daemon/src/idle.rs`)

An agent left running is not free: it holds a PTY, an OS thread, a VT engine
with up to `terminal.scrollback_lines` rows behind it, and, for the providers
that poll, a live connection. A day of work leaves a tail of sessions that
finished their task hours ago.

Every live session carries a runtime-only `last_activity_at` (see
[domain.md](./domain.md)). A sweeper thread evaluates the `[sessions]` policy
against it every 30 seconds, and the decision itself is a pure function of
(session, now, whether a client is attached, whether it was already warned), so
it is unit-testable without a PTY.

| Key | Default | Effect |
|-----|---------|--------|
| `idle_warn_after_secs` | `1800` | Inactivity after which a session is reported **once per quiet spell**. The only rule on by default. |
| `idle_stop_after_secs` | `0` (off) | Inactivity after which a session is **stopped**. Raised to `idle_warn_after_secs` if set below it. |
| `idle_stop_attached` | `false` | Whether a stop may end a terminal a client has open. |
| `idle_include_shells` | `false` | Whether the rules apply to shells; a shell at a prompt is a resting state, not a leak. |
| `long_running_warn_after_secs` | `0` (off) | Age since creation after which a session is reported once, however busy. Never stops anything. |

Two properties are worth stating plainly:

- **Nothing is stopped unless the user asks.** Ending someone's process is never
  inferred from a timer, so `idle_stop_after_secs` ships at `0`.
- **A stop is an ordinary stop.** It goes through the normal kill path (SIGTERM
  to the process group, SIGKILL after `kill_grace_ms`), announces itself with a
  notice, and leaves the session `Exited` and restartable. No client
  special-cases an idle death, because there is nothing special about it.

The sidebar renders the same idleness as a dim `45m` on the session row once it
passes ten minutes — ambient information, well below the notice threshold.

## Discovered history

Agent CLIs keep their own transcripts, and a run started in a plain shell — or
before the project was added to Forge — is still a run the user remembers.
`crates/daemon/src/external_agents.rs` reads them into
`domain::ExternalAgentSession` values that ride along on every `GetSnapshot`.

Discovery is per **directory**, not per project: each project root *and* each of
its worktrees is scanned, which is what attributes a run to the worktree it
actually happened in.

| Provider | On disk | Read from it |
|----------|---------|--------------|
| Claude Code | `~/.claude/projects/<slug>/<sessionId>.jsonl`, `<slug>` = the directory with `/` and `.` collapsed to `-`; subagents in `<sessionId>/subagents/*.jsonl` | recorded `cwd`/`gitBranch`, min/max `timestamp`, latest `aiTitle`, turn count, the last assistant `text` block and its `message.model` |
| opencode ≥ 1.17 | `~/.local/share/opencode/opencode.db` (and any `opencode-<suffix>.db` beside it), read by `crates/daemon/src/opencode_db.rs` | `session` rows whose `directory` is the scanned one: `id`, `title`, `time_created`/`time_updated`, turns from `message`, the newest non-synthetic `text` part of the last assistant message, and that message's `modelID`. `parent_id` marks a subagent run — counted against its parent, never listed — and `time_archived` hides one the user put away |
| opencode < 1.17 (legacy tree) | `~/.local/share/opencode/storage` (XDG on every platform): `project/<hash>.json` → `worktree`, sessions in `session/<hash>/`, conversation in `message/<sessionID>/` and `part/<messageID>/` | `id`, `title`, `directory`, `time.created`/`updated`, turn count, the last assistant message's text parts and its `modelID`. A session with a `parentID` is a subagent run, counted against its parent rather than listed |

Four rules keep it honest and cheap:

- **Best effort, never an error.** An unreadable file, a malformed line or a
  missing directory yields fewer results.
- **Bounded.** Only the 60 most recently modified transcripts per directory are
  read, and a Claude transcript is streamed line by line — parsed only for the
  records the scan reads anything out of, with the `timestamp` recovered from
  the raw line so first/last activity stays exact anyway.
- **Off the lock.** `Daemon::snapshot` releases the core lock before calling
  `discover`, so transcript IO never blocks daemon state.
- **Cached.** `external_agents::Cache` reuses a pass for 10 s, keyed on a
  fingerprint of the directories it covered. `GetSnapshot` is answered on every
  reconnect, so without it the GUI re-read every transcript on disk to learn
  what it learned seconds earlier. Adding a project or worktree changes the
  fingerprint and rescans at once; the TTL only covers re-asking the same
  question, and an agent session ending invalidates it outright.

The recorded directory is verified against the one being scanned: Claude's slug
is lossy (distinct paths can collide onto one folder) and one opencode project
spans a repository and its worktrees, so a transcript from elsewhere is dropped
rather than mis-attributed.

### opencode's two layouts

opencode moved session storage into SQLite in 1.17 and stopped writing the JSON
tree, so **both are read**. The database comes first and the tree second, and a
session id already listed from a database is skipped on the way through the
tree: an upgraded machine has the same run in both places, and the copy the tree
holds stopped being updated the day opencode was upgraded.

The database belongs to another program. It is opened read-only with
`PRAGMA query_only = ON` on top, and every failure — no file, an unfamiliar
schema, a locked database, a malformed JSON blob — resolves to "no sessions from
here", so the tree is still scanned and the panel degrades to what it can read.
Columns are probed with `PRAGMA table_info` before they are selected, which is
what lets one build read a schema older or newer than the one it was written
for. Because the column stores whatever path opencode was started with, the
directory is matched against both the path Forge holds and its canonical form —
`/tmp` against `/private/tmp` is otherwise the difference between a full history
and an empty panel.

For a session recorded in the database there is no per-session file left, so
`transcript_path` is the database itself: that is where the run is recorded, and
what "reveal the transcript" has to point at.

## Token analytics (§16.2)

`ProviderUsage` says how much allowance is left; `agents::collect_analytics`
says what was actually spent. It reads the same transcript stores discovery
walks, but for the one thing discovery ignores — the usage record on every
billed turn — and aggregates them into a `domain::UsageAnalytics` for the
settings screen's Stats & Usage page.

| Provider | Read from it |
|----------|--------------|
| Claude Code | every `type: "assistant"` record's `message.usage`: `input_tokens`, `output_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, the `output_tokens_details.thinking_tokens` subset, and the `cache_creation` 5m/1h split that prices a cache write. `message.model` names the model, `message.id` dedupes |
| Codex CLI | each `token_count` event's `info.total_token_usage`, which is **cumulative**: a turn is the difference between consecutive events. `cached_input_tokens` is subtracted out of `input_tokens`, because Codex counts cache reads inside the input while Claude keeps them apart. The model comes from the preceding `turn_context` |

OpenCode and Cursor are absent for the same reason they declare no
`UsageSource`: OpenCode bills through whichever model provider it is configured
with, and Cursor records nothing locally to count.

The same four rules as discovery apply, with different constants:

- **Best effort.** A malformed line is skipped, never an error.
- **Bounded**, and it says so: the 500 most recently modified transcripts per
  provider are read and the rest are counted into `UsageAnalytics::skipped`. A
  bounded scan that looks exhaustive is worse than one that admits its bound.
- **Off the lock**, behind `daemon::usage_stats::Cache` with a 60 s TTL keyed on
  the window, so re-rendering the page re-reads nothing and asking a *different*
  question is not answered with the previous one's numbers.
- **Cheap where it can be.** Only files modified inside the window are opened,
  and only lines containing `"usage"` (or `"token_count"`) are parsed — most of
  a transcript is user turns and tool traffic.

Two counting rules are worth stating because getting either wrong silently
doubles a number:

- A resumed Claude session copies earlier records into the new transcript, so
  turns are deduped on `message.id` across every file in the scan.
- Thinking tokens are output tokens. `TokenTotals::reasoning` is reported beside
  `output`, never added into `total()`.

Cost is an estimate from `usage/pricing.rs`: Anthropic list prices per model
family in micro-dollars, with the documented cache multipliers (a read at 0.1x
input, a five-minute write at 1.25x, a one-hour write at 2x). A model with no
published price in this build contributes **nothing** and increments
`unpriced_turns`, which is what lets the page say the cost is a floor rather
than quietly under-reporting. Inventing a rate for an unknown model would turn a
missing number into a wrong one.

## Tests

`crates/agents` unit-tests detection and launch with `agents::test_support`
(fake executables on a temp PATH: correct marker, wrong marker, hanging probe,
an environment overlay that must not touch `TERM`, and directory creation).
`crates/daemon/tests/scenario_profiles.rs` covers §13.4 end to end: the child
process really receives the profile's environment and arguments, its config
directory is created first, a restart repeats it, and every invalid profile is
refused at save time.
`crates/test-support::fake_agent` provides the same for daemon-level tests.
Note that real daemon startup runs `--version` on whichever agent CLIs are
installed on the machine, so workspace tests are not fully hermetic.
