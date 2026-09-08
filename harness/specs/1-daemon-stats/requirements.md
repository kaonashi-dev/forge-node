# Requirements — daemon-stats (feature 1)

Wire `forge-daemon stats` so it queries an already running daemon over IPC and
prints real runtime statistics. Out of scope: `dump`, PTY/IPC bytes/s, queue
depth, resync counters, and the §16.2 UI (`GetUsageAnalytics`).

## Requirements

### R1 — Synchronous stats read over IPC

The system SHALL expose `Request::GetStats`, which the daemon answers
synchronously with `Response::DaemonStats` containing: the session count per
`SessionState`, the number of open terminals, the number of connected clients
and the daemon uptime (`uptime_secs` since `Daemon::started_at`).

**Evidence:** `cargo test -p daemon --test integration stats_against_running_daemon -- --exact`

### R2 — The CLI prints the metrics against a live daemon

WHEN `forge-daemon stats` runs with a daemon listening on the resolved socket,
the system SHALL print to stdout the sessions per state, the number of
terminals, the connected clients and the uptime, and exit with code 0.

**Evidence:** `cargo test -p daemon --test integration stats_against_running_daemon -- --exact`

### R3 — No daemon: clear failure

IF there is no daemon listening on the resolved socket THEN `forge-daemon stats`
SHALL exit with a non-zero code and print a clear message on stderr (no stack
trace).

**Evidence:** `cargo test -p daemon --test integration stats_without_daemon_exits_nonzero -- --exact`

### R4 — It does not start a daemon nor touch the singleton lock

WHEN `forge-daemon stats` is invoked, the system SHALL limit itself to
connecting to the socket and asking for `GetStats`: it SHALL NOT call
`daemon::run`, SHALL NOT acquire the singleton lockfile and SHALL NOT create a
new daemon process.

**Evidence:** `cargo test -p daemon --test integration stats_does_not_touch_singleton_lock -- --exact`

### R5 — E2E case in integration.rs

The system SHALL include in `crates/daemon/tests/integration.rs` at least one
case that starts its own daemon (temporary socket / in-memory DB, `start_daemon`
pattern) and checks the output of `forge-daemon stats` against that socket.

**Evidence:** `cargo test -p daemon --test integration stats_against_running_daemon -- --exact`

## Traceability acceptance → R<n>

| Acceptance | R<n> |
| --- | --- |
| `forge-daemon stats` against a running daemon prints sessions per state, number of terminals, connected clients and uptime | R1, R2, R5 |
| With no daemon running, it exits with a code != 0 and a clear message on stderr (not a stack trace) | R3 |
| The command does not create a new daemon nor touch the singleton lockfile | R4 |
| There is a case in crates/daemon/tests/integration.rs that starts its own daemon and checks the output | R5, R2 |
