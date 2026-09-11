# Agent providers

An **agent is a terminal session running a known CLI** (ADR-006). Forge does
not speak to agents over an API; it spawns their interactive TUI in a PTY and
renders it like any shell. Everything provider-specific lives in
`crates/agents` (principle P2) — no other crate branches on a provider id.

Plan references: §7.5, §7.6, §13. ADR-007 (declarative provider registry).

## Built-in providers (`builtins.rs`)

| id | Display name | Binary candidates (in order) | Probe | Marker | Config directory | Resume | Prompt | Read-only |
|----|--------------|------------------------------|-------|--------|------------------|--------|--------|-----------|
| `claude` | Claude Code | `claude` | `--version`, 3 s | — | `CLAUDE_CONFIG_DIR` | `--resume <id>` | positional | `--permission-mode plan` |
| `codex` | Codex CLI | `codex` | `--version`, 3 s | — | `CODEX_HOME` | `resume <id>` (subcommand) | positional | `-s read-only` |
| `opencode` | OpenCode | `opencode`, `opencode2` | `--version`, 3 s | — | `OPENCODE_CONFIG_DIR` + `XDG_DATA_HOME` | `--session <id>` | `--prompt <text>` | `--agent plan` |
| `cursor` | Cursor CLI | `agent`, `cursor-agent` | `--version`, 3 s | output or unambiguous basename must contain `cursor` | — | — | positional | `--mode ask` |
| `grok` | Grok | `grok` | `--version`, 3 s | output must contain `grok` | `GROK_HOME` | `--resume <id>` | positional | `--permission-mode plan` |

All five are interactive TUIs and take no `default_args`. Each capability flag
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
in the directory the original run happened in. It is also scoped to an
*account*: `ExternalAgentSession::profile_id` names the profile whose store the
transcript was read from, and the history panel launches with that profile,
because the default account has never heard of an id recorded under a profile's
config directory. See `docs/ui.md` for the history panel that sends them.

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

A **profile** is a named way to start a known provider — its own binary, its
own account and its own arguments. It replaces the hand-written shell wrapper
people write to run two accounts:

```fish
function claude-personal
    set -lx CLAUDE_CONFIG_DIR "$HOME/.claude-personal"
    mkdir -p "$CLAUDE_CONFIG_DIR"
    command claude $argv
end
```

A profile is *not* a provider: it borrows the descriptor's icon, detection,
binary candidates and usage source, and overrides only how the process starts.
`domain::AgentProfile` carries `{ provider_id, name, executable, config_dir,
args }` — four fields, because a profile that could set anything was a second,
worse copy of the agent's own configuration file. The bare provider is the
implicit profile (`profile_id: None`).

**Provider-specific knowledge stays here** (P2): each descriptor declares how
it is pointed at a directory, as data.

```rust
ConfigDirSpec {
    vars: vec!["CLAUDE_CONFIG_DIR"],
    help: "A separate account: its own login, settings, history and plugins.",
}
```

`CLAUDE_CONFIG_DIR` is the documented account switch for Claude Code (settings,
session history and plugins move with it); `CODEX_HOME` is its Codex
equivalent, and `usage/codex.rs` already reads credentials through it. OpenCode
needs two variables for the one directory, because it splits what the other two
keep together: `OPENCODE_CONFIG_DIR` moves the directory `opencode.json` is
read from — and with it the agents, commands and plugins declared beside it —
while `XDG_DATA_HOME` is what moves `opencode/auth.json` and the session
storage. Setting only one would switch half an account, so a profile sets both.
Cursor CLI declares none: it documents no configuration switch, so its profiles
change the binary and the arguments only, and a directory saved for it is
refused rather than stored where nothing would read it.

**Where the directory actually is** is a runtime question, so it is resolved at
launch and not at save (`AgentProfile::resolve_config_dir`): an absolute path is
used as typed, `~/…` is expanded, and anything else is relative to the
launching user's `$HOME`. That last rule is the whole point — the daemon's own
working directory is `/` under launchd, where a `.claude-personal` created
relative to it fails on a read-only file system. `ensure_config_dir` then runs
the wrapper's `mkdir -p` with mode `0700` before the launch. Nothing matches on
a provider id to do either.

A profile's `executable` is resolved the same way a shell would resolve it
(`agents::resolve_executable`): an absolute path as given, a bare name looked up
on the resolved login-shell PATH. That is the second shape a personal profile
takes — no directory at all, because a `claude-personal` wrapper on `PATH`
already carries the account. A shell *alias* is not a program and cannot be
launched: what runs is the script or symlink it stands for.

**A profile is an account, and everything that reads a provider's own
directories reads all of them.** Moving the config directory moves the
transcripts with it, so both scans take the profile list and walk one store per
account, dropping duplicates by canonical path:

- `external_agents::discover` (the history panel) scans `<config dir>/projects`
  for Claude Code and `<config dir>/opencode` for OpenCode — the latter because
  the directory *is* `XDG_DATA_HOME` for the launch — alongside `~/.claude` and
  `~/.local/share/opencode`. Its cache fingerprint includes the profile
  directories, so a saved profile shows its history at once.
- `agents::collect_analytics` (the usage page) sums `<config dir>/projects` for
  Claude and `<config dir>/sessions` for Codex with the default account's. The
  page is what the machine spent, not what one login did, so the totals are the
  sum; saving or deleting a profile drops the cached scan.

The allowance meter (`usage::collect`) reads one account at a time, and the
daemon sweeps all of them: the default login plus every profile that moved the
config directory, each probed against an environment whose `ConfigDirSpec`
variables point at that account (`agents::env_for_config_dir`, the same helper
the launch uses, so a reading and a launch cannot disagree about which login
they mean). Every reading carries the `profile_id` it came from, and the status
bar draws one meter per account rather than one per provider.

On macOS the Keychain holds a single Claude login — the default account's — so a
profile whose own directory has no `.credentials.json` reports **nothing**
rather than falling back to it. A meter under a profile's name showing the
default account's numbers is the failure this split exists to prevent; note
that a fresh `CLAUDE_CONFIG_DIR` also means the CLI itself starts logged out
until that account is signed in.

### Rules

- A profile belongs to one provider. Launching Codex with a Claude profile is
  `InvalidRequest`, not a silently ignored field.
- Names are unique per provider, case-insensitively (a unique index in SQLite,
  surfaced as `Conflict`): two profiles cannot render as the same menu entry.
- A config directory for a provider that declares no `ConfigDirSpec` is
  `InvalidRequest`: a directory nothing is ever told about would look saved and
  change nothing.
- `domain::RESERVED_PROFILE_VARS` (`TERM`, `TERMINFO`, `COLORTERM`,
  `FORGE_SESSION_ID`, `FORGE_WORKSPACE`) is what Forge owns of the child's
  environment. A profile can no longer name a variable at all; the constant is
  the contract every descriptor's `ConfigDirSpec` is tested against.
- Inside a Forge-launched PTY, `forge-daemon` is on `PATH` and
  `FORGE_SESSION_ID` is set. Agents cite or spawn other sessions through Forge,
  not peer-to-peer: `forge-daemon session list|read|spawn-child` and
  `forge-daemon context send|list`. Full model, GUI gestures and framed paste:
  [session-context.md](./session-context.md).
- A profile with its own `executable` is version-probed when it is **saved**,
  exactly like a `SetProviderExecutable` override, so the daemon still never
  launches a binary no probe accepted. At launch time only its presence and
  executable bit are re-checked.
- Deleting a profile never touches sessions it already started; `RestartSession`
  re-applies the profile, and refuses if it is gone rather than silently
  restarting under a different account.
- Arguments are stored as a list and passed one by one; the editor parses its
  text like a command line (whitespace separates — a newline is just more
  whitespace — quotes group, backslash escapes), so `--model opus` is two
  arguments. It writes them back one flag per line, quoting only an argument
  that really contains whitespace and giving that one a line of its own.
  `normalizeArgs` repairs the profiles the previous editor saved, where a whole
  line became one argument. No shell ever runs any of it: `$HOME`, `*` and `;`
  reach the agent as characters.

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

`build_launch_with_config_dir(descriptor, req, env, config_dir)` is the same
builder plus a profile's directory, written to every variable the descriptor's
`ConfigDirSpec` names, *after* the terminal hints. `build_launch` is that
function with no directory, so there is one code path.

`FORGE_SESSION_ID` and `FORGE_WORKSPACE` are deliberately **not** added here:
the crate has no `SessionId`. The daemon injects them when it spawns the PTY
(`core.rs`), keeping this builder pure.

## Attention (needs-you)

The rail's `needs-you` marker is driven by ASCII BEL. Providers that do not
emit BEL on a permission / question prompt get a Forge-owned adapter at launch
(`crates/agents/src/attention.rs`). Assets live under
`<data_dir>/agent-plugins/` (idempotent, content-hashed).

| Provider | Injection | Signal |
|----------|-----------|--------|
| `opencode` | `OPENCODE_CONFIG_CONTENT` → `forge-attention.js` | `permission.asked` / `question.asked` → BEL |
| `claude` | `--settings` → Forge JSON (does **not** rewrite `~/.claude`) | `PermissionRequest` + `Notification(permission_prompt\|…)` → `forge-ring-bell.sh` |
| `codex` | `-c tui.notifications=["approval-requested"]` + `notification_method="bel"` + `condition="always"` | TUI BEL on approval only |
| `cursor` | Merge `beforeShellExecution` / `beforeMCPExecution` into `~/.cursor/hooks.json` (FORGE-gated) | Best-effort — Cursor has no permission-prompt hook |
| `grok` | Write `~/.grok/hooks/forge-attention.json` (FORGE-gated) | `Notification(permission_prompt)` → BEL |

`forge-ring-bell.sh` is a no-op without `FORGE_SESSION_ID`, drains stdin, and
never fails the agent. Editing OpenCode's `attention` config or Claude's
`preferredNotifChannel` does nothing here — Forge does not go through those
channels. `OPENCODE_PURE=1` / `--pure` disables the OpenCode plugin.

`OPENCODE_CONFIG_CONTENT` is a launch detail, not a terminal-contract variable:
it is not in `RESERVED_PROFILE_VARS`, and it composes with a profile's
`OPENCODE_CONFIG_DIR`. Provider-shaped knowledge stays in `crates/agents`; the
daemon only supplies the asset directory (P2).

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
a profile directory that must not touch `TERM`, a bare executable name found on
PATH, and directory creation).
`crates/daemon/tests/scenario_profiles.rs` covers §13.4 end to end: a relative
directory lands under `$HOME` and an absolute one where it says, the child
process really receives it along with the profile's arguments, a wrapper named
like a shell alias is found on PATH, a restart repeats all of it, and every
invalid profile is refused at save time.
`crates/test-support::fake_agent` provides the same for daemon-level tests.
Note that real daemon startup runs `--version` on whichever agent CLIs are
installed on the machine, so workspace tests are not fully hermetic.
