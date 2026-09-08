# Tasks — daemon-stats (feature 1)

- [x] **T1 — `GetStats` / `DaemonStats` protocol**  
      Add `Request::GetStats`, the `DaemonStats` + `SessionsByState` structs and
      `Response::DaemonStats` in `crates/protocol`. MessagePack round-trip in
      the existing test module.  
      Covers: R1

- [x] **T2 — Handler in the daemon**  
      In `Daemon::handle_request`, answer `GetStats` with `collect_stats()`
      (counts under `inner`, then `registry.client_count()`, `uptime_secs` since
      `started_at`). Respect the lock order; no `.await`.  
      Covers: R1

- [x] **T3 — Client**  
      Add `Client::get_stats` (or an equivalent match) that asks for `GetStats`
      and returns `DaemonStats` / `ClientError`.  
      Covers: R1

- [x] **T4 — Socket resolution + `stats` CLI**  
      Honour `FORGE_SOCKET` in the resolution used by the CLI; implement
      `daemon::stats()` (connect → get_stats → print to stdout); wire
      `Command::Stats` in `main.rs` (remove the `not wired yet` stub). Errors →
      clear message, `Err`, exit ≠ 0. No `run` and no lockfile.  
      Covers: R2, R3, R4

- [x] **T5 — Integration: live daemon**  
      In `crates/daemon/tests/integration.rs`: `stats_against_running_daemon` —
      `start_daemon`, `FORGE_SOCKET`, spawn `CARGO_BIN_EXE_forge-daemon stats`,
      assert exit 0 and stdout with sessions-per-state, terminals, clients and
      uptime.  
      Covers: R2, R5  
      Verify: `cargo test -p daemon --test integration stats_against_running_daemon -- --exact`

- [x] **T6 — Integration: no daemon + lock untouched**  
      `stats_without_daemon_exits_nonzero` and
      `stats_does_not_touch_singleton_lock` in the same `integration.rs`.  
      Covers: R3, R4  
      Verify:
      - `cargo test -p daemon --test integration stats_without_daemon_exits_nonzero -- --exact`
      - `cargo test -p daemon --test integration stats_does_not_touch_singleton_lock -- --exact`

- [x] **T7 — Full gate**  
      Run `./init.sh` (without `--fast`) and leave it green.  
      Covers: R1–R5
