# Design — daemon-stats (feature 1)

## Summary

`forge-daemon stats` stops being a no-op: it resolves the socket, talks to the
daemon through `client::Client` (blocking std, Hello as `ClientKind::Gui`), asks
for `GetStats` and formats the answer to stdout. No daemon → stderr + exit ≠ 0.
Without touching `run` nor the singleton lock.

## Files to touch

| Path | Crate | Change |
| --- | --- | --- |
| `crates/protocol/src/request.rs` | protocol | Add `Request::GetStats` (unit, no fields). |
| `crates/protocol/src/response.rs` | protocol | Add `DaemonStats` + `Response::DaemonStats(DaemonStats)`. |
| `crates/protocol/src/lib.rs` | protocol | Re-export `DaemonStats` if the rest of the response types are re-exported; the module's existing MessagePack round-trip tests. |
| `crates/daemon/src/core.rs` | daemon | Arm in `handle_request`: `GetStats` → build `DaemonStats` (lock `inner` → then `registry`, no `.await`). |
| `crates/daemon/src/paths.rs` | daemon | Honour `FORGE_SOCKET` when resolving the socket (same order as the GUI), so tests and scripts can point at a temporary `d.sock` without the user's singleton. |
| `crates/daemon/src/lib.rs` | daemon | Add `pub fn stats() -> anyhow::Result<()>`: resolve socket → `Client::connect` → `request(GetStats)` → print → Ok/Err. |
| `crates/daemon/src/main.rs` | daemon | Replace the `Command::Stats` stub with `daemon::stats()`. |
| `crates/client/src/ipc.rs` | client | Optional helper `Client::get_stats(&self) -> Result<DaemonStats, ClientError>` (symmetric to `get_usage_analytics`); otherwise the CLI uses `request` + a direct match. Prefer the helper so the `Response::DaemonStats` arm is matched in a single place. |
| `crates/client/src/lib.rs` | client | Re-export `DaemonStats` if other crates need it through `client` (the `forge-daemon` binary already depends on `client`/`protocol`). |
| `crates/daemon/tests/integration.rs` | daemon | Three cases: `stats_against_running_daemon`, `stats_without_daemon_exits_nonzero`, `stats_does_not_touch_singleton_lock`. |

**Do not touch:** `dump`, the `stats.rs` UI / `GetUsageAnalytics`, `persistence`
/ migrations, `domain` (except if `SessionState` / `Timestamp` are reused from
protocol).

## New signatures

### protocol

```rust
// request.rs — inside Request
GetStats,

// response.rs
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStats {
    /// Seconds from `Daemon::started_at` to the instant of the answer.
    pub uptime_secs: u64,
    /// Live/known sessions grouped by `domain::SessionState`.
    pub sessions_by_state: SessionsByState,
    /// `Inner.terminals.len()` — open PTYs.
    pub open_terminals: u64,
    /// `ClientRegistry::client_count()` — current IPC connections
    /// (includes the client asking for stats).
    pub connected_clients: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsByState {
    pub starting: u64,
    pub running: u64,
    pub exited: u64,
    pub failed: u64,
    pub orphaned: u64,
}

// Response
DaemonStats(DaemonStats),
```

Hierarchy: wire types in `protocol` (like `ProviderInfo`); they are neither
persisted domain state nor SQLite columns. `SessionState` is counted in the
daemon and projected onto fixed fields — there is no need to serialize the enum
in the map (which avoids surprises with `#[non_exhaustive]`).

### daemon

```rust
// lib.rs
pub fn stats() -> anyhow::Result<()>;

// core.rs — inside handle_request
Request::GetStats => Ok(Response::DaemonStats(self.collect_stats())),
```

`collect_stats` (internal name is free): under the `Inner` mutex it counts
`sessions` per `state` and `terminals.len()`; afterwards, without holding the
core lock across an await, it reads `registry.client_count()` (`inner ->
registry` order). `uptime_secs` = `now - self.started_at` in seconds, saturated
to `u64`.

### client

```rust
impl Client {
    pub fn get_stats(&self) -> Result<protocol::DaemonStats, ClientError>;
}
```

### CLI / paths

- `paths::socket_path()` (or a helper used only by the `stats`/`info` clients):
  if `std::env::var_os("FORGE_SOCKET")` is set, use it; otherwise the current
  resolution (`$TMPDIR/forge` / `XDG_RUNTIME_DIR` / `/tmp/forge-$UID` +
  `daemon.sock`, ADR-004 budget).
- `stats()` does **not** call `lockfile::acquire` nor `run()`.
- connect / request errors: `anyhow` with a short message such as
  `forge-daemon stats: daemon not running (…)` to stderr through the existing
  `main`/`anyhow`; **no** `unwrap`/`expect` on the happy path. Exit code ≠ 0
  when `stats()` returns `Err`.
- stdout output: stable text, readable at a glance, e.g. lines `uptime_secs=…`,
  `sessions.starting=…`, …, `open_terminals=…`, `connected_clients=…` (the exact
  format is the implementer's choice, but the tests must anchor on concrete
  substrings/fields).

## Shape of errors

| Situation | Behaviour |
| --- | --- |
| Socket missing / `connect` fails | `Err` → clear message on stderr, exit ≠ 0, no backtrace by default |
| HelloReject / protocol error | Same: `Err` propagated, exit ≠ 0 |
| Unexpected answer (not `DaemonStats`) | `ClientError` / `anyhow`, exit ≠ 0 |
| Live daemon, zero sessions | exit 0; counts at zero; uptime ≥ 0 |

## Invariants (AGENTS.md)

- **Lock order** `inner -> registry`; do not hold the core lock across an
  `.await` nor over network/filesystem I/O in `collect_stats`.
- **Local synchronous request** like `GetWorkspaceDiff` / `GetUsageAnalytics`:
  the answer travels in the body, **without** `Ack`+event, without broadcast.
- **No SQLite migration**: `DaemonStats` is runtime-only on the wire; nothing in
  `Store` and no columns.
- **Blocking std client**: the CLI binary may block on `Client::request`; there
  is no GUI toolkit thread here.
- **`#[non_exhaustive]` enums**: the `Request`/`Response` matches in
  daemon/client gain the new arm + conservative wildcards where they already
  exist.
- **Do not confuse this with §16.2**: do not reuse `GetUsageAnalytics` nor
  `usage_stats::Cache`.
- **`dump` stays a no-op** in this feature.

## E2E tests

The `start_daemon` pattern in `integration.rs` (socket under `/tmp`, in-memory
DB). To exercise the **binary**:

1. `std::process::Command::new(env!("CARGO_BIN_EXE_forge-daemon"))` with
   `FORGE_SOCKET=<temp>/d.sock`, args `["stats"]`, capturing
   stdout/stderr/status.
2. No daemon: `FORGE_SOCKET` pointing at a path that does not exist → status ≠
   0, non-empty stderr and no `stack backtrace` string (nor the `not wired yet`
   stub).
3. Singleton lock: record `paths::lock_path()` (existence/mtime/content) before
   and after a failed or successful `stats` through a temporary `FORGE_SOCKET`;
   it must stay the same. The command must not create `daemon.lock` next to the
   test socket.

Make sure in the `daemon` package's `Cargo.toml` that the integration test can
see the binary (`CARGO_BIN_EXE_forge-daemon` already applies to bins of the same
package).

## Discarded alternative

**Deriving the metrics only from `GetSnapshot` + `Client::daemon_info().started_at`
without `GetStats`.** Discarded because: (1) the snapshot does not reliably
expose `client_count` nor the size of `Inner.terminals` (sessions ≠ PTYs; IPC
clients do not live in the snapshot); (2) it would force the CLI to download the
whole state just for a histogram and an uptime; (3) the explore and `plan.md`
§22 already mark stats as a runtime read of the daemon/`ClientRegistry`, not as
a view of the snapshot. A dedicated request keeps the same shape as the rest of
the synchronous reads and leaves room to add bytes/s later without breaking the
CLI.
