# Orchestration

`forgectl` is a client of the running `forge-daemon`. It does not start a
second daemon and it does not run jobs itself. The daemon keeps the ledger and
is the only enforcer of the rails. A controller — a human, or an ordinary Forge
agent session — decides what happens next.

Source: `crates/domain/src/orchestration` (pure policy), `crates/daemon/src/orchestration`
(effects), `crates/forge-ctl` (the binary).

## Nouns

| Term | Meaning |
|------|---------|
| **Run** | One objective, one controller, one integration workspace, one board. |
| **Task** | A unit of work inside a run, with dependency edges. A task is ready only when every dependency is accepted or integrated. |
| **Attempt** | One assignment of a task to a session. Only `forgectl report` from that attempt's own session settles it. |
| **Board** | Run-scoped versioned JSON. `state set` and `state del` require `--if-version`; `0` creates a missing key. |
| **Controller** | The session allowed to accept, reject, integrate, and cancel. A human caller (no `FORGE_SESSION_ID`) is always allowed. |

Identity inside a worker is only the injected environment: `FORGE_SESSION_ID`,
`FORGE_RUN_ID`, `FORGE_TASK_ID`, `FORGE_ATTEMPT_ID`. There is no hidden run.

## What the daemon will not do

- Choose the next task, retry a failed attempt, or resume workers after a restart.
- Merge a pull request into the project's default branch. `agent_may_integrate`
  defaults to `pr`: an agent controller may open one pull request and may merge
  into the integration branch. Nothing in Forge merges that pull request.
- Settle an attempt because it went idle, missed a heartbeat, timed out, or the
  process exited. Exit without `report` marks the attempt lost and raises
  attention.
- Accept a report from another session or from a superseded attempt (exit 5).

A daemon restart marks live attempts lost (`daemon_restarted`) and an active run
interrupted. `run resume` starts a new controller seeded from the ledger; it
also revives an active run whose controller session has ended, and never
replaces a live one. Run messages, including unanswered questions, outlive the
sessions that sent and received them.

## Placement

A write task defaults to its own managed worktree, branched from the run's
integration head (`forge/task-<slug>-<n>`). Read-only attempts may share a
checkout. At most one writing attempt may be live in a workspace. The other
rails, overridable under `[orchestration]` except the always-on caps, are at
most 4 live attempts per run, 32 tasks per run, run depth 2, and 3 attempts
per task. The feature ships enabled.

## Channels

There is no free-form memory store. Controllers and workers share four things:
stored messages (the body is never written into a PTY), reports (summary capped
at 16 KiB, optional result file capped at 1 MiB), the board, and the integration
branch. At most one one-line pointer is typed, and only while activity is idle
and nobody is blocked in `inbox --wait`, which returns on unread mail only. Waiting, starting, and unknown suppress
the pointer. A separate carriage return follows the line.

`run wait` is level-triggered: if the condition already holds it returns the
triggering items immediately. It, `ask --wait`, `inbox --wait` and `state watch`
block on daemon events rather than polling; `run wait` also re-reads every 5 s
because `waiting` and `stalled` attention appear when a clock crosses a
threshold, which no event announces. Every wait defaults to 600 s, or to the
global `--timeout` when one is given; a malformed `--timeout` exits 2.

## CLI contract

`--json`, or `FORGECTL_JSON=1`, prints one object on stdout.
Success is `{"ok":true,"result":…}`. Failure is
`{"ok":false,"error":{"code","message","details","next"}}` with `next` as argv.
Exit codes: 0 ok, 1 other refusal, 2 usage, 3 unreachable or protocol mismatch,
4 not found, 5 conflict or precondition, 6 policy, 124 wait timeout. A repeated
mutation `--request-id` returns the stored result; one that arrives while the
first is still running is refused with exit 5 rather than run twice. An uncertain timeout exits 3
with `details.uncertain: true`.

`guide` and `schema` are served by the binary. `hook` always exits 0 and does
not write SQLite. A CLI connection is not sent terminal activity, bell,
clipboard, editor-frame, or usage broadcasts.

`pr draft` is outside the run ledger. It is how a session that the GUI's
Open PR tab launched in a *draft* task hands back its pull request: title,
branch, commit subject (plus optional `--commit-body`) as flags, the body on
stdin or `--body-file`. The daemon checks that `FORGE_SESSION_ID` names a known
session, caps each field at 64 KiB, and broadcasts `PullRequestDraftReady` for
that session's workspace; it stores nothing. The GUI shows the text for editing
and only then sends `CommitAndOpenPullRequest`, which commits everything once
on a new branch when the checkout is on its default (never on the default
itself), pushes, and runs `gh pr create --assignee @me`.

`forge-daemon session` and `forge-daemon context` still run and print a
deprecation hint pointing at `forgectl`.

## Dispatch between sessions and projects

Session dispatch does not create a run or a task. It launches independent graph
roots, including in another project; `FORGE_SESSION_ID` is reported as
`spawned_by`, never used as a cross-project parent.

```sh
forgectl --json project list
forgectl --json session list --project api --kind agent --active
forgectl --json session spawn --project api --provider claude --profile Work \
  --name auth-impl --worktree auth-impl --base main \
  --model opus --effort high --prompt-file plan/auth.md
forgectl session rename api/auth-impl auth-review
forgectl session wait api/auth-review --for idle --timeout 600
forgectl session read api/auth-review --lines 200
forgectl session changes api/auth-review
forgectl session send api/auth-review --queue --text 'Review the failing test.'
forgectl --json session shell --project api --name checks
forgectl session exec api/checks --text 'cargo test -p client'
```

`--project` accepts an exact name (case-insensitive), root path or UUID, the
same way in every command that takes one (`run start` and `run list` included).
Paths resolve canonically, so symlinks and platform aliases name the same project.
`--workspace <uuid|prefix>` uses an existing checkout and conflicts with `--worktree`.
Creating a worktree waits for `WorkspaceCreated` with the ordinary request
timeout; it does not reload a snapshot in a loop. `--base` requires `--worktree`.
Every check that needs no daemon round trip (name, project, workspace, parent,
provider capabilities) runs before the worktree is created. A launch the
daemon still refuses after that reports the checkout in
`details.worktree_created`, and `next` starts with
`forgectl workspace remove <id>`; nothing removes it automatically.

Called from inside a Forge session (`FORGE_SESSION_ID` set), `--project` may
be omitted: the session lands in the caller's project **and checkout**, unless
`--workspace` or `--worktree` says otherwise. `session spawn` may also omit
`--provider`: it then uses the caller's provider and, unless `--profile` is
given, its profile (the same account). A `FORGE_SESSION_ID` this daemon does
not know is a usage error rather than "no caller".

`--prompt`/`--prompt-file` and `--context`/`--context-file` together are
capped at 64 KiB for a launch: the prompt travels as one process argument, not
as a paste, so the 8 KiB `send` budget does not apply. Context follows the
prompt under `Context handed over from Forge session "<title>" (<id>):`; the
caller writes that summary, since a TUI transcript is not a usable record of a
conversation.

`--name` travels in the creating request (`title` on `CreateAgentSession` and
`CreateShellSession`), so a session is never created and then left unnamed by
a failed rename. `--role` tags the session, and `--parent <ref|self>` places it
under a session of the **same** project (the daemon refuses another project's
parent); without `--parent` it stays an independent root. `--resume` passes a
provider-owned conversation id and is refused for a provider that cannot resume.

`session list` filters by `--project`, `--kind agent|shell|editor|task` and
`--active`; each entry adds `active`, `ended`, `exit_code`, `signal`, `role`
and `parent` beside the display strings. `session read --lines N` bounds the
transcript by lines as well as by `--max-bytes`. `session changes` is
`GetSessionChanges`: what the session changed since its base commit.
`session restart`, `session close` and `session signal --signal
int|term|hup|kill [--foreground]` expose the daemon's lifecycle requests;
`--foreground` signals the terminal's foreground process group, as a typed
Ctrl-C would, because a job a shell started is not in the session's own group.
`workspace list`/`workspace remove [--force]` list and remove checkouts, and
`action list`/`action run <label|id>` run an *approved* project action
(`RunAction`) — a repository file is never executed from here.
`--prompt` and `--prompt-file <path|->` are exclusive, as are `--context` and `--context-file`; see the 64 KiB launch budget below.
`--profile` accepts a case-insensitive name or UUID for the selected provider.
`--read-only`, `--model`, and `--effort` require descriptor support
([agents.md](./agents.md)). A restart retains model, effort and read-only mode
for the daemon's lifetime; it never re-sends the initial prompt.

Spawn returns `result: {session_id, terminal_id, project, workspace, title,
provider, spawned_by}` without reloading a snapshot to guess the ids. Names
are trimmed and cannot be empty. Session references in `show`, `read`, `kill`,
`rename`, `send`, and `wait` accept a UUID, at least eight UUID-prefix characters,
an exact case-insensitive title, or `project/title`. Titles need not be unique.
An ambiguous reference exits 5 with candidate ids/prefixes, project, title,
provider, state and activity. Resolving names reads one cached snapshot per
command. Use returned UUIDs for unattended scripts.
`session list` keeps `kind` and `state` as display strings (including exit
details); `session show` returns the structured domain session.

`self` and `parent` are session references relative to `FORGE_SESSION_ID`
and win over a title with the same text. A child reports back with
`session send parent --queue --file report.md`: the report waits in the
parent's one queued slot and is submitted when the parent is hook-confirmed
idle, so a planner can learn that its coder finished without polling.
`session watch [<ref>...] [--project <name>]` follows sessions by events and
prints one JSON line per state or activity change (`snapshot`, `updated`,
`removed`), ending when every named session has ended or at its deadline
(exit 124); unlike other commands it streams lines rather than one object.

`session wait --for idle|waiting|working|exit` takes a comma-separated list
and returns when the first condition holds, with `matched` naming it. A
controller waiting for a worker to finish waits for `idle,waiting,exit`:
`idle` alone sits out the whole timeout when the worker stops to ask a
question. The command checks the initial snapshot and then
blocks on `SessionUpdated`, not polling. Only agents report activity, so these
states are refused with exit 5 for any other kind; `--for exit` waits for any
session to end and returns its state (exit code or signal). The default
deadline is 600 seconds; `--timeout` accepts seconds or a duration such as
`10s`. Timeout exits 124, removal exits 4, and an agent that ends before the
activity it was waited for exits 5. **Idle is a turn boundary, not
proof of success**: inspect the transcript, diff and verification before acting.
Silence alone never changes the daemon's activity state.

### Safe prompt delivery

`session send <ref> --text <text>`, `--file <path|->`, or a positional `-`
accepts at most 8 KiB and removes control characters except LF/TAB. It always
uses paste framing, never arbitrary keys. Without bracketed paste, LF and TAB
become spaces so they cannot approve a permission dialog. `--no-submit` omits
Enter even when the target is idle.

| Policy | Behavior |
|--------|----------|
| Default (`IfIdle`) | Refuse unless the live agent is `idle` with hook evidence. |
| `--queue` | Keep one bounded prompt until hook-confirmed idle; deliver immediately if already idle. A second prompt is `Conflict` (exit 5), never a replacement. |
| `--now` | Paste in any activity state; submit only in hook-confirmed idle. **Never approve permissions or answer questions with this command.** |

The result contains `{context_id, delivered, queued, submitted}`. `delivered`
means the PTY write succeeded, not that the agent accepted or completed the
work. Text entered into an idle prompt changes it to `working`, including a
non-submitted draft, preventing a second automatic submission. A queue belongs
to the exact terminal generation and is discarded on exit, restart or removal.
An uncertain write is consumed and never automatically replayed. The context
envelope audits accepted, sanitized text, not proof of delivery; SQLite writes
use a separate connection outside the core lock. Explicit retry after a
timeout can duplicate input: inspect the terminal first. Unlike ledger
mutations, session dispatch is not deduplicated by `--request-id`.

`forgectl guide sessions` serves this contract to an agent. A parent-folder
planner and a Raycast example live in [multi-repo.md](./multi-repo.md).

### Shell commands

`session exec <ref> --text <line>` (or `-` for stdin, one trailing newline
dropped) sends `SendShellCommand`: one line of at most 8 KiB, typed with paste
framing and followed by Enter. A line containing any control character — LF,
CR, TAB included — is refused, not stripped, because stripping would join two
commands or change a completion. The daemon delivers it only while the shell's
own process group owns the terminal's foreground (`tcgetpgrp` on the master);
while a job runs (an editor, a pager, an agent started by hand) the request
exits 5, since that job would read the line and its Enter as keys. The check
runs again under the PTY writer lock right before writing, the same ordering
`send` uses, and the accepted line is audited as a context envelope. There is
no completion signal for a command: read the transcript, or end the shell with
`exit <code>` and `session wait --for exit`. Text a person already typed at
that prompt is not cleared and would prefix the line.

### Spec runs

`forgectl spec run <spec>` launches a planner session whose prompt is the brief
`domain::spec_run::compose_planner_prompt` writes; `forgectl spec brief <spec>`
prints the same brief for the calling session to follow as the planner. The
spec is a Claude artifact link (`claude.ai`, `claude.site`), another link, a
Markdown file or the text itself (`-` reads stdin). Forge classifies it and
never fetches it: an artifact needs the planner's own login and artifact tool.
A value that looks like a path but is not a file exits 4 rather than becoming
a one-line spec.

The palette's **Run Spec…** is the same launch from the GUI: the host composes
the brief with the same function and starts the planner in the checkout on
screen. A relative path resolves against that checkout, the Markdown file on
screen is prefilled, and a refusal (missing file, unknown coder) is a notice
before anything starts.

The planner validates every criterion (context, and a check that proves it),
asks the person every open point at once with its question selector, writes a
plan with no open question, launches the coder with `session spawn --parent
self --role executor`, waits for `idle,waiting,exit`, reruns the validations
itself, sends numbered findings, and hands what remains to the person after
`--rounds` (default 3). It never commits, pushes or merges.

`--preset claude` codes with Claude at `medium`; `--preset mix` codes with
Cursor, or with `--coder opencode`. A preset is a provider and an effort;
`--coder` replaces both. The planner is Claude at `high` unless `--planner`
and `--planner-effort` say otherwise. Provider, effort and model are checked
against the detected descriptors before anything launches; Cursor applies an
effort only with `--coder-model`, since its effort is a model suffix.

### MCP stdio

`forgectl mcp` exposes `list_projects`, `list_providers`, `list_workspaces`,
`list_sessions`, `show_session`, `read_session`, `session_changes`,
`spawn_session`, `spawn_shell`, `exec_shell`, `send_prompt`, `wait_session`,
`rename_session`, `signal_session`, `kill_session`, `restart_session`,
`close_session`, `list_actions`, `run_action`, `run_spec` and `spec_brief`.
A positional `-` is refused, since stdin is the request stream. It invokes the same CLI
handlers in-process, not a child `forgectl`. Input lines are capped at 64 KiB;
tool schemas reject extra arguments and key-injection options, and every
positional value is passed after `--`. Up to 8 `tools/call` requests run at
once, each on its own daemon connection (a shared one would let concurrent
waits consume each other's events); a ninth waits for a free slot, so a long
`wait_session` (maximum 600 s) no longer stalls the calls behind it. Responses
can therefore arrive out of request order; match them by `id`.

Configure it explicitly; launching an agent does not silently grant dispatch
tools, especially to a pull-request reviewer. Use the absolute installed
`forgectl` path and pass `--socket <path>` before `mcp` if the host does not
inherit `FORGE_SOCKET`. For Claude, a launch-local file supplied with
`--mcp-config <file>` can contain:

```json
{"mcpServers":{"forge":{"command":"/absolute/path/forgectl","args":["mcp"]}}}
```

`--mcp-config` is variadic, like Claude's `--add-dir`: do not leave it as the
last option in a profile before Forge's positional prompt. Follow it with a
separate option, or use Claude's ordinary project MCP configuration instead.

Codex's own `config.toml` accepts:

```toml
[mcp_servers.forge]
command = "/absolute/path/forgectl"
args = ["mcp"]
```

OpenCode V2's server configuration (not `cli.json`) accepts:

```json
{"mcp":{"servers":{"forge":{"type":"local","command":["/absolute/path/forgectl","mcp"]}}}}
```

An OpenCode shared service may not inherit a Forge PTY's environment; specify
the daemon socket explicitly in its command. This server has no filesystem
write tool, but dispatching a writing agent is itself a mutation, and
`exec_shell` runs a command line in any shell session at its prompt: review the
provider's MCP permissions before exposing it to a read-only planner.
