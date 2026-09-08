# Explore: `forge-daemon stats` / `dump` (feature 1 — daemon-stats)

Read-only survey. No crates edited.

## Where the CLI lives

| Piece | Location |
| --- | --- |
| Binary | `crates/daemon/Cargo.toml` → `[[bin]] name = "forge-daemon"`, `path = "src/main.rs"` |
| CLI entry | `crates/daemon/src/main.rs` — clap `Cli` / `Command` |
| Runtime lib (run/info only) | `crates/daemon/src/lib.rs` — `run()`, `info()`; **no** `stats`/`dump` helpers |
| Dev wrapper for dump | `scripts/dev` case `dump` → `cargo run -p daemon --bin forge-daemon -- dump --json` |
| Docs saying no-ops | `AGENTS.md` (Ground Truth), `README.md`, `docs/README.md`, `docs/development.md`, `execution.md` (§22 / ADR-004) |

### `Command` variants (`crates/daemon/src/main.rs`)

```rust
enum Command {
    Run,
    Info,
    Dump { json: bool },  // `--json`; doc: §10.4; "Not yet wired"
    Stats,                // doc: §22; "Not yet wired"
}
```

Default with no subcommand: `Command::Run` via `unwrap_or(Command::Run)`.

## What they print and exit code

Both arms are no-ops that **do not connect** to a running daemon, open the socket, or touch `Daemon` state:

| Subcommand | stderr | return | process exit |
| --- | --- | --- | --- |
| `Dump { .. }` | `forge-daemon dump: not wired yet. See execution.md.` | `Ok(())` | **0** |
| `Stats` | `forge-daemon stats: not wired yet. See execution.md.` | `Ok(())` | **0** |

Notes:

- Message is on **stderr** (`eprintln!`), not stdout.
- `Dump`'s `json: bool` is ignored (`Dump { .. }`).
- `main() -> anyhow::Result<()>`; successful `Ok(())` ⇒ exit code 0 (same pattern as a second `run` when the lock is held).

Sibling docs: `docs/development.md` lines ~69–70; `plan.md` §22 (intended metrics); `execution.md` still lists both as unimplemented.

## Not the same as UI “Stats & Usage”

`apps/tauri / `GetUsageAnalytics` / `daemon::usage_stats::Cache` are **§16.2 token/cost analytics**, unrelated to §22 daemon runtime `stats`. Do not conflate.

## Runtime data that could feed §22 `stats`

Target design (`plan.md` §22): uptime, sessions-by-state, PTY/IPC bytes/s, attached terminals, per-client queue depth, resyncs emitted.

### Already present (usable or nearly)

| Metric | Where | Symbols |
| --- | --- | --- |
| **Uptime** | `Daemon::started_at: Timestamp` set in `Daemon::load` / start path; also written into lockfile and sent on hello | `core::Daemon.started_at`; `lib::run` local `started_at`; `lockfile::acquire(..., started_at)`; `protocol::hello::HelloAck.started_at` (filled in `server.rs` HelloAck) |
| **Sessions by state** | `Inner.sessions: HashMap<SessionId, Session>` under core mutex | Count by `domain::session::SessionState::{Starting, Running, Exited, Failed, Orphaned}` (`is_active` / `is_terminal` helpers exist) |
| **Live terminals** | `Inner.terminals: HashMap<TerminalId, TerminalRuntime>` | `len()` = live PTYs; fields `id`, `child_pid` marked “Reserved for diagnostics/stats (§22)” (`terminal::TerminalRuntime`) |
| **Attached / watched terminals** | `ClientRegistry` | `has_subscribers(TerminalId)`, `subscriber_count(TerminalId)`, `subscribed_terminals()` |
| **Connected clients** | `ClientRegistry::client_count()` | Doc comment already says “Used by `stats` (§22)” |
| **Per-client behind / subscriptions** | private `ClientHandle { subscriptions, behind, tx }` | Resync-needed set exists; no public “queue depth” or “resyncs emitted” counter yet |
| **Queue capacity constant** | `registry::CLIENT_QUEUE_CAPACITY = 256` | Depth would need `tx.len()` (or similar) exposed; not wrapped today |

### Not present yet (plan wants them; would need new instrumentation)

- PTY bytes/s (no byte counters on `pty_loop` / `pump_terminal`)
- IPC bytes/s (framing read/write paths do not tally)
- Aggregate “resyncs emitted” counter (`route_terminal_delta` sends `TerminalResync` but does not count)

### Dump sibling (for context)

- Intended: stream protocol as JSON (`plan.md` / ADR-004; `protocol::framing::to_json_string` exists for that).
- `protocol::hello` notes dump CLI would connect as `ClientKind::Gui` for now.
- Today: same no-op path as stats; helper unused by the binary.

## Bottom line

`stats` and `dump` live only in the `forge-daemon` clap surface in `crates/daemon/src/main.rs`. They print a fixed “not wired yet” line to stderr and exit **0**. Uptime, session-state histogram, terminal map size, subscriber/attached sets, and client count already exist on a live `Daemon`/`ClientRegistry`; bytes/s and resync totals do not.
