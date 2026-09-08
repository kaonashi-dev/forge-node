# Explore: client↔daemon IPC + E2E precedent (daemon-stats)

Feature: `1` — daemon-stats. Read-only; `crates/` was not touched.

## How an external process talks to a running daemon

### Transport

- Unix domain socket. MessagePack framing + a BE `u32` prefix (`protocol::framing`).
- §9.2 handshake: `ClientMessage::Hello` → `DaemonMessage::HelloAck` (or `HelloReject`).
- Then request/response correlated by `request_id`, plus unsolicited `DaemonEvent`s.

### Canonical client: `crates/client/src/ipc.rs`

| API | Role |
|-----|------|
| `Client::connect(socket_path: &Path, client_version)` | `UnixStream::connect`, Hello as `ClientKind::Gui`, spawns the reader thread |
| `Client::request(Request) -> Result<Response, ClientError>` | Blocking; oneshot per `request_id` |
| `Client::request_timeout(...)` | The same with a deadline |
| `Client::events() -> flume::Receiver<DaemonEvent>` | Unbounded channel; it has to be drained |
| `Client::daemon_info()` | Data from the `HelloAck` (`instance_id`, versions, `started_at`) |

Notes:

- A **blocking `std`** client, not Tokio (AGENTS.md invariant).
- Today it **always** talks as `ClientKind::Gui`. There is no CLI variant in the enum; `docs/protocol.md` / `hello.rs` say the debug CLI (`dump --json`) will also use `Gui` for now. Only `Gui` and `Unknown` exist.
- The socket path is supplied by the caller; the `client` crate does not resolve runtime paths.

### Protocol: is there a `GetStats`?

**No.** There is no `GetStats` / `Dump` / similar in `crates/protocol/src/request.rs`.

The closest and **different** thing (the §16.2 UI, not §22 runtime):

- `Request::GetUsageAnalytics { window_days }` → `Response::UsageAnalytics(Box<UsageAnalytics>)`
- A local synchronous read of transcripts; not uptime / PTY bytes / IPC queues.

Other synchronous reads useful as a precedent of shape, not of semantics: `GetSnapshot`, `GetWorkspaceDiff`, `ListBranches`, `ListAgentProviders`.

`Response` has no runtime "daemon stats" arm either.

### The `forge-daemon` CLI today (`crates/daemon/src/main.rs`)

| Subcommand | Current behaviour | Opens a socket? |
|------------|-------------------|-----------------|
| `run` (default) | `daemon::run()` — singleton lock + bind + serve | Yes (bind) |
| `info` | `daemon::info()` — prints paths/config and exits | **No** |
| `dump --json` | stub: eprint `"not wired yet"` + `Ok(())` | **No** |
| `stats` | stub: eprint `"not wired yet"` + `Ok(())` | **No** |

Helpers already in place for when it gets wired:

- `protocol::framing::to_json_string` — meant for `dump --json` (§10.4).
- `registry::Registry::client_count` — with an explicit comment "Used by `stats` (§22)".
- Fields in `terminal.rs` "Reserved for diagnostics/stats (§22)".

Target design (`plan.md` §22): `stats` must report uptime, sessions per state, PTY bytes/s, IPC bytes/s, attached terminals, queue depth, resyncs. That does **not** come out of `GetSnapshot` nor of `GetUsageAnalytics`.

`scripts/dev dump` → `cargo run -p daemon --bin forge-daemon -- dump --json` (the same stub).

## Socket path resolution

### Daemon / `info` / `run` — `crates/daemon/src/paths.rs`

- `paths::socket_path()` → `resolve_runtime_dir()?.join("daemon.sock")`.
- Preferred: macOS `$TMPDIR/forge`, Linux `$XDG_RUNTIME_DIR/forge`.
- Fallback: `/tmp/forge-$UID`.
- ADR-004 cap: the full path &lt; 100 bytes; if it does not fit, error (do not truncate).
- Lock alongside it: `daemon.lock` (`lock_path()`).
- It does **not** read `FORGE_SOCKET`. There is **no** socket override in `config.toml`.

`info()` only resolves and prints; it does not talk to a live daemon.

### GUI — `apps/tauri `socket_path()`

- It **does** honour `FORGE_SOCKET` first.
- Then the same preferred/fallback logic as the daemon (duplicated, not via `daemon::paths`).

Implication for test CLI↔daemon runs: today a `forge-daemon stats` wired to `paths::socket_path()` would hit the **user's singleton**, not a temporary `d.sock`, unless an override (env or flag) aligned with the GUI is added.

## E2E precedent: your own daemon without touching the singleton

The tests **never** run `forge-daemon run` nor acquire the user's `daemon.lock`. They start the library in-process over a temporary socket under `/tmp`.

### A) `crates/daemon/tests/integration.rs` — §21

The `start_daemon` / `start_daemon_with` pattern:

1. `tempfile::tempdir_in("/tmp")` → `…/d.sock` (ADR-004 / `sun_path`).
2. `persistence::Db::open_in_memory()`.
3. `Daemon::start(db, cfg, worktrees_root, instance_id, version)`.
4. A thread with a Tokio runtime: `daemon::server::bind(&sock)` + `serve(...)`.
5. Poll until the socket file exists.
6. `Client::connect(&td.socket, "itest")` + `request` / `events`.

Test config: `sessions.shell = "/bin/sh"` (not the developer's login shell).

Typical teardown: `Request::StopDaemon { kill_sessions: true }` (or a crash via `request_shutdown` in scenarios); the accept loop unlinks the socket.

There is also a hand-written `SlowClient` (`UnixStream` + framing) for backpressure — a precedent for a "non-`Client`" client over the same socket.

### B) `crates/daemon/tests/common/mod.rs` — §19 scenarios

`Harness` / `TestDaemon` — the same temp-socket pattern, but:

- A shared **file** DB (`forge.db`) so it can be restarted.
- A synthetic login shell + a hermetic `PATH` (`bin/`).
- `Harness::boot()` / `boot_with(|cfg| …)` → `Daemon::start` + bind/serve.
- `TestDaemon::connect(client_version)` → `Client::connect(&self.socket, …)`.
- Reusable helpers: `wait_for`, `poll_until`, `add_main_workspace`, `create_shell_session`, `attach`, `stop_daemon`, etc.
- Reading **without a socket** when the daemon has already "crashed": `daemon.handle_request(Request::GetSnapshot)` over the abandoned `Arc<Daemon>`.

Used by `scenario_*.rs` (usage, profiles, idle, restart, …).

### C) Client unit tests — `crates/client/src/ipc.rs` `mod tests`

A `scripted_server` over a `UnixListener` in `/tmp/.../s`: a fake daemon that only does a HelloAck + scripted answers. Useful for the shape of the wire, not for real runtime.

### What is **not** there

- No E2E test spawns the `forge-daemon` binary (`Command::new`, `CARGO_BIN_EXE_*`, `assert_cmd`).
- No test checks the stdout of `dump`/`stats`/`info` against a live daemon.
- No test uses the user's lockfile/socket.

For "checking the output of a binary/CLI" the closest precedent is:

1. In-process boot (`Harness` / `start_daemon`) → temp socket.
2. Talk with `Client` (not the CLI).
3. Or, for fake scripts on `PATH`, `write_executable` + `Command::output` (agents/git), **without** IPC to the daemon.

A future test of `forge-daemon stats` as an external process would have to be
**new**: spawn the binary + a socket override (non-existent in the daemon today)
+ a stdout assert; or test the logic through `Client::request(GetStats)` against
the harness and leave the CLI as a thin wrapper.

## Verdict for the daemon-stats feature

| Question | Finding |
|----------|---------|
| How does an external process talk? | `Client::connect(path)` + Hello + `request`/`events` over the UDS. |
| Is there a `GetStats` in protocol? | **No.** |
| Is `GetUsageAnalytics` useful for §22? | **No** — it is agent usage, a different feature. |
| Can `stats`/`dump` be "local CLI only" without IPC? | **`info` can** (paths/config). **`stats`/`dump` cannot**, if they must reflect an **already running** daemon: connecting to the socket is required. A separate CLI process does not see the runtime's `Mutex<Inner>`. |
| Is a new `Request` needed? | **Yes, almost certainly for `stats`**, if the `plan.md` §22 metrics are implemented (queues, bytes/s, resyncs, server process uptime). Adding only a CLI that prints stubs or reads `GetSnapshot` would cover a poor fraction. `dump` is a stream of messages in JSON on connect (a passive client + `to_json_string`), not necessarily a Request. |
| Reusable E2E precedent | `Harness` / `start_daemon`: in-process daemon + `/tmp/.../d.sock` socket + `Client`; **no** singleton. For the CLI binary's stdout there is no precedent; add a socket override (`FORGE_SOCKET` or a flag) aligned with the GUI. |

## Key paths

- `crates/client/src/ipc.rs` — connect / request
- `crates/protocol/src/{request,response,hello,framing}.rs`
- `crates/daemon/src/{main,lib,paths,server,registry}.rs`
- `crates/daemon/tests/{integration.rs,common/mod.rs,scenario_*.rs}`
- `apps/tauri — `FORGE_SOCKET`
- `docs/development.md`, `plan.md` §22, `execution.md` (stats/dump not wired)
