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
interrupted. `run resume` starts a new controller seeded from the ledger.

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
and nobody is blocked in `inbox --wait`. Waiting, starting, and unknown suppress
the pointer. A separate carriage return follows the line.

`run wait` is level-triggered: if the condition already holds it returns the
triggering items immediately.

## CLI contract

`--json`, or `FORGECTL_JSON=1`, prints one object on stdout.
Success is `{"ok":true,"result":…}`. Failure is
`{"ok":false,"error":{"code","message","details","next"}}` with `next` as argv.
Exit codes: 0 ok, 1 other refusal, 2 usage, 3 unreachable or protocol mismatch,
4 not found, 5 conflict or precondition, 6 policy, 124 wait timeout. A repeated
mutation `--request-id` returns the stored result. An uncertain timeout exits 3
with `details.uncertain: true`.

`guide` and `schema` are served by the binary. `hook` always exits 0 and does
not write SQLite. A CLI connection is not sent terminal activity, bell,
clipboard, editor-frame, or usage broadcasts.

`forge-daemon session` and `forge-daemon context` still run and print a
deprecation hint pointing at `forgectl`.
