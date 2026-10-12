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
triggering items immediately.

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
forgectl --json session list
forgectl --json session spawn --project api --provider claude --profile Work \
  --name auth-impl --worktree auth-impl --base main \
  --model opus --effort high --prompt-file plan/auth.md
forgectl session rename api/auth-impl auth-review
forgectl session wait api/auth-review --for idle --timeout 600
forgectl session read api/auth-review
forgectl session send api/auth-review --queue --text 'Review the failing test.'
```

`--project` accepts an exact name (case-insensitive), root path or UUID.
Paths resolve canonically, so symlinks and platform aliases name the same project.
`--workspace <uuid>` uses an existing checkout and conflicts with `--worktree`.
Creating a worktree waits for `WorkspaceCreated` with the ordinary request
timeout; it does not reload a snapshot in a loop. `--base` requires `--worktree`.
`--prompt` and `--prompt-file <path|->` are exclusive, bounded to 8 KiB.
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

`session wait --for idle|waiting|working` checks the initial snapshot and then
blocks on `SessionUpdated`, not polling. The default deadline is 600 seconds;
`--timeout` accepts seconds or a duration such as `10s`. Timeout exits 124,
removal exits 4, and an ended session exits 5. **Idle is a turn boundary, not
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

### MCP stdio

`forgectl mcp` exposes `list_sessions`, `spawn_session`, `send_prompt`,
`wait_session`, `read_session`, and `rename_session`. It invokes the same CLI
handlers in-process, not a child `forgectl`. Input lines are capped at 64 KiB;
tool schemas reject extra arguments and key-injection options. Requests are
processed sequentially: a long `wait_session` occupies the server until its
deadline (maximum 600 s); use a short timeout compatible with the host.

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
write tool, but dispatching a writing agent is itself a mutation: review the
provider's MCP permissions before exposing it to a read-only planner.
