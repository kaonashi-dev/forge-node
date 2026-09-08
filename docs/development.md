# Development

## Prerequisites

- Rust 1.89 (pinned in `rust-toolchain.toml`; `rustup show` installs it).
- `git` and a C toolchain (bundled SQLite compiles from source).
- [Bun](https://bun.sh) 1.1+ — runs the subagent harness CLI
  (`scripts/harness`, `./init.sh` step 3). No other JS runtime is needed;
  the harness is TypeScript executed directly by Bun, with no build step.
- macOS or Linux. The code uses Unix sockets, PTYs and signals directly;
  Windows is not a target.

## Commands

```sh
scripts/dev check                # the canonical gate (default for bare `scripts/dev`):
                                 #   cargo fmt --all -- --check
                                 #   cargo clippy --workspace --all-targets -- -D warnings
                                 #   cargo test --workspace
cargo build --workspace
cargo test  --workspace
cargo test -p <crate>            # one crate
cargo deny check                 # license/advisory policy (deny.toml); install with
                                 # `cargo install cargo-deny --locked`
```

CI (`.github/workflows/ci.yml`) runs the same three steps on `macos-latest` and
`ubuntu-latest`, plus a separate `cargo-deny` job via `EmbarkStudios/cargo-deny-action`, which needs
no compilation. `cargo deny check` is not part of `scripts/dev check`, so run it
locally after touching dependencies.

Packaging lives in `scripts/package-macos` and `scripts/package-linux`. Neither
is run by `scripts/dev` or by CI; run them by hand and see `--help` for the
signing/notarization and runtime-dependency contracts.

For moving a build to another machine, `scripts/dist` wraps those packagers
into one transferable tarball per platform with an embedded `install.sh`:
`scripts/dist build` produces `dist/release/forge-<ver>-<os>-<arch>.tar.gz`,
and on the target machine `tar xzf` + the bundled `./install.sh` installs it
(quarantine stripped, architecture checked, dropped into `/Applications`).
`scripts/dist ship <user@host>` builds, copies over scp and prints the remote
one-liner. The receiving machine needs only `tar` — no checkout, no Rust.

Useful exact invocations:

```sh
cargo test -p client --lib ipc::tests::connect_request_and_event_round_trip -- --exact
cargo test -p daemon --test integration end_to_end_shell_session -- --exact
cargo test -p daemon --test integration                    # all non-ignored E2E cases
cargo test -p daemon --test integration installed_agent_tuis_render_and_accept_input -- --exact --ignored
cargo test -p daemon --test integration key_byte_to_grid_delta_p95_is_under_33ms -- --exact --ignored
cargo test -p daemon --test integration macos_vim_htop_color_and_alt_screen_smoke -- --exact --ignored
cargo test -p terminal-core --test golden plain_text -- --exact
```

## Running the daemon by hand

```sh
scripts/dev daemon               # cargo run -p daemon --bin forge-daemon (RUST_LOG=debug)
scripts/dev daemon info          # print resolved paths + effective config, then exit
forge-daemon run                 # acquire the singleton lock, bind, serve (default)
forge-daemon dump --json         # NOT wired yet — no-op
forge-daemon stats               # NOT wired yet — no-op
```

The Tauri app connects to the per-user daemon or starts the adjacent
`target/debug/forge-daemon`, adds the current directory if the store has no
project, and creates or reuses a live shell. Run `scripts/dev daemon` separately
only when backend logs need to remain in the foreground.

`scripts/dev daemon` uses the **real** per-user paths (config, DB, logs,
worktrees). The daemon is a singleton per user via an advisory `flock` on
`daemon.lock`; a second `run` exits **0** while the first holds the lock, so
exit code 0 does not prove a new daemon started — check the log.

Paths (macOS shown; Linux uses `$XDG_RUNTIME_DIR`, `$XDG_DATA_HOME`,
`$XDG_CONFIG_HOME`):

| What | Path |
|------|------|
| socket / lock | `$TMPDIR/forge/daemon.sock`, `daemon.lock` (full path < 100 bytes, else `/tmp/forge-$UID/`) |
| database | `~/Library/Application Support/Forge/app.db` |
| worktrees | `~/Library/Application Support/Forge/worktrees/<project-id>/<slug>` |
| logs | `~/Library/Application Support/Forge/logs/daemon.log` (rotated) |
| config | `~/Library/Application Support/Forge/config.toml` |

`config.toml` is optional; a missing file uses defaults; changes need a
restart (no hot reload). See [`config.example.toml`](./config.example.toml).
`RUST_LOG` overrides `daemon.log_level`.

## Where things are

| Want to change… | Look in |
|-----------------|---------|
| a domain type or state-machine rule | `crates/domain` → [domain.md](./domain.md) |
| a request/response/event | `crates/protocol` + the handler in `crates/daemon/src/core.rs` → [protocol.md](./protocol.md) |
| terminal rendering data, input mapping, delta/seq rules | `crates/terminal-core`, `crates/terminal-input`, `crates/daemon/src/terminal.rs`, `crates/client/src/store.rs` → [terminal.md](./terminal.md) |
| a provider, detection, launch args | `crates/agents` only → [agents.md](./agents.md) |
| git behavior, worktree safety | `crates/git-service`, `core.rs` workspace handlers → [worktrees.md](./worktrees.md) |
| the schema | `crates/persistence/src/migrations.rs` (append only) → [persistence.md](./persistence.md) |
| paths, config, logging, singleton | `crates/daemon/src/{paths,config,logging,lockfile}.rs` |
| Tauri frontend layout, panels, terminal | `apps/tauri/src` → [ui.md](./ui.md) |
| theme tokens | `apps/tauri/src/theme/tokens.ts` |

Dependency direction is one-way and enforced by `Cargo.toml`:
`forge-tauri → client → {protocol, terminal-input} → domain` and
`daemon → {agents, git-service, persistence, terminal-core} → domain`.

## Testing notes

- Do not keep a hand-maintained workspace test total here; `scripts/dev check`
  is the source of truth as coverage changes.
- `terminal-core` uses `insta` golden snapshots in
  `crates/terminal-core/tests/snapshots/`; accept changes with
  `cargo insta review` (requires `cargo install cargo-insta`).
- `crates/daemon/tests/integration.rs` starts its **own** daemon with an
  in-memory DB and a temp socket — do not run a developer daemon for it. It
  needs a writable `/tmp`, a real shell/PTY and real `git`, and fails
  (not skips) without them.
- `git-service` integration tests **return early when `git` is absent**, so a
  green package run does not prove Git coverage ran.
- Tests are not fully hermetic: shell-environment tests invoke the user's
  login shell, and daemon startup probes whichever agent CLIs are installed
  (`claude --version`, …).
- The ignored Phase 0 reference smokes above are intentionally local: they need
  installed/authenticated agent TUIs, `vim`, `htop`, a real PTY, and macOS.
  Linux Wayland/X11 runtime validation is deferred and is not covered by a
  Docker/headless substitute.
- Terminal unit tests use `test-support::FakePtyBackend`; `fake_agent`
  provides fake provider executables; `temp_repo` creates throwaway Git repos.

## Conventions

- `rustfmt` defaults and `clippy -D warnings` are mandatory (the gate).
  TypeScript in `apps/tauri` uses the same idea: `oxlint` (correctness),
  `oxfmt`, and `tsc --noEmit`. Do not add `any`.
- Protocol and domain enums are `#[non_exhaustive]`: add a wildcard arm in
  cross-crate matches.
- Mutations return `Response::Ack`; state changes are broadcast as events.
- Never hold the daemon core lock across `.await`; lock order
  `inner → registry`.
- Persisted enum tags are stable strings — renaming a variant is a migration.
- Keep `AGENTS.md` as the short list of invariants for contributors and
  AI agents; keep `execution.md` as the log, not the spec.
- Comments carry a constraint the types cannot (rejected alternative, unit,
  lock/IO exception, budget). They do not restate the next line, narrate
  history, or copy `plan.md`. One sentence is the default; essays live in
  `docs/` or an ADR. Full rule: `AGENTS.md` § *Comments And Code*.
