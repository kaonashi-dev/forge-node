# Impl — feature 1 daemon-stats

## R<n> → evidence

| R | Evidence |
| --- | --- |
| R1 | `cargo test -p protocol --lib request::tests::get_stats_round_trips_through_messagepack -- --exact`; `cargo test -p protocol --lib response::tests::daemon_stats_round_trips_through_messagepack -- --exact`; `cargo test -p daemon --test integration stats_against_running_daemon -- --exact` |
| R2 | `cargo test -p daemon --test integration stats_against_running_daemon -- --exact` (exit 0 + stdout `uptime_secs` / `sessions.*` / `open_terminals` / `connected_clients`) |
| R3 | `cargo test -p daemon --test integration stats_without_daemon_exits_nonzero -- --exact` |
| R4 | `cargo test -p daemon --test integration stats_does_not_touch_singleton_lock -- --exact` |
| R5 | the same `stats_against_running_daemon` case in `crates/daemon/tests/integration.rs` (`start_daemon` + the `CARGO_BIN_EXE_forge-daemon` binary) |
| Gate | `./init.sh` (without `--fast`) → green in round 0 (`fmt + clippy + test`) |

## Files that belong to feature 1 (daemon-stats)

The exact list of the stats scope. The reviewer must evaluate **only** these
paths (+ this feature's harness files below).

### protocol
- `crates/protocol/src/request.rs` — `Request::GetStats` + round-trip test
- `crates/protocol/src/response.rs` — `DaemonStats`, `SessionsByState`, `Response::DaemonStats` + round-trip test
- `crates/protocol/src/lib.rs` — re-export `DaemonStats`, `SessionsByState`

### daemon
- `crates/daemon/src/core.rs` — `collect_stats()` + the `GetStats` arm in `handle_request`
- `crates/daemon/src/server.rs` — `GetStats` span name
- `crates/daemon/src/paths.rs` — `FORGE_SOCKET` through `socket_path` / `socket_path_from`
- `crates/daemon/src/lib.rs` — `pub fn stats()`
- `crates/daemon/src/main.rs` — `Command::Stats` → `daemon::stats()`
- `crates/daemon/tests/integration.rs` — three E2E cases (`stats_*`)
- `crates/daemon/Cargo.toml` — `client` becomes a runtime dependency (needed by `stats()`)

### client
- `crates/client/src/ipc.rs` — `Client::get_stats`
- `crates/client/src/lib.rs` — re-export `DaemonStats`

### harness (this feature)
- `harness/features.json`
- `harness/progress/current.md`
- `harness/progress/impl_1.md`
- `harness/progress/review_1.md`
- `harness/specs/1-daemon-stats/` (`requirements.md`, `design.md`, `tasks.md`)
- `harness/progress/explore_1_ipc_tests.md`, `harness/progress/explore_1_stats_cli.md` (round 0 exploration notes)

## Outside crates[] / mixed tree

The working tree **already had unrelated changes** before (and during) feature 1.
`git diff HEAD` and `git status` mix that WIP with the daemon-stats work; **those
files are not part of the feature 1 scope**.

Examples of WIP **not** attributable to daemon-stats (do not revert / do not
evaluate as part of this feature's C6):

- `apps/tauri/`, `apps/tauri/src/theme/`, `crates/domain/`, `crates/persistence/` (migrations), `crates/git-service/`, `crates/agents/`, `crates/terminal-core/`, `crates/fs-service/`
- Inside declared crates but **unrelated to stats**: e.g. `crates/daemon/src/pull_requests.rs`, `crates/daemon/src/usage_stats.rs`, `crates/daemon/src/external_agents.rs`, `crates/daemon/src/config.rs`, `crates/daemon/src/terminal.rs`, non-stats `scenario_*` tests, `crates/client/src/store.rs`, `crates/protocol/src/event.rs`, docs/`AGENTS.md`/`plan.md`/`execution.md` from other lines of work

Some files in the stats list (e.g. `core.rs`, `integration.rs`, `request.rs`) may
**also** contain unrelated WIP hunks in the same path. The review contract is: to
judge only the regions / behaviour of `GetStats` / `DaemonStats` /
`collect_stats` / the `stats` CLI / the `stats_*` tests; the rest of the hunks in
those files is not feature 1 scope.

No unrelated WIP was reverted, stashed or deleted: destroying it is forbidden by
the lead in round 1. The C6 fix is documentary — the explicit scope is above.

## Out of scope (deliberately, per the spec)

- `dump` stays a no-op
- The §16.2 UI / `GetUsageAnalytics` / `usage_stats` untouched **as part of this feature**
- No PTY/IPC bytes/s, no queue depth and no resync counters
- No SQLite migrations / no changes in `domain` or `persistence` **because of stats**

## Round 1

Answer to `review_1.md` (C6):

1. Feature marked `in_progress` with `review_rounds=1` untouched; after this doc → `in_review`.
2. The tree was **not** isolated with `git restore` / stash / clean: the previous WIP (ui, fs-service, PRs, migrations, etc.) stays intact at the lead's request.
3. `impl_1.md` updated: the exact list of daemon-stats files per crate; the "Outside crates[] / mixed tree" section honestly describes the unrelated WIP and asks the reviewer to evaluate only the stats list (+ this feature's harness files).
4. No micro-fix of stats code; no re-run of the workspace (documentary/harness change only).
