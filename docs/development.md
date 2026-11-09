# Development

## Prerequisites

- macOS or Linux. The daemon uses Unix sockets, PTYs and signals; Windows is
  not supported.
- Rust: the root workspace is pinned to 1.89.0; `apps/ui` is pinned separately
  to 1.99.1 (Rune needs edition 2024). `rustup` reads each toolchain file.
- `git` and a C toolchain (bundled SQLite compiles from source).
- A linker-compatible macOS SDK: `scripts/macos-sdk` picks one this `ld` can
  parse (the highest `xcrun` SDK can name an architecture the installed linker
  does not know). `scripts/dev` exports it; bare `cargo` on macOS needs
  `SDKROOT="$(scripts/macos-sdk)"`.
- No database service and no Node/Bun runtime.

## Gate

```sh
scripts/dev check          # root fmt + clippy + test, then apps/ui on 1.99.1
./init.sh                  # toolchain check, then the same gate
./init.sh --fast           # skips the cargo block
```

`scripts/dev check --root-only` skips the Rune client. CI runs the full gate on
macOS. `cargo fmt --all` at the root does not cover `apps/ui`; `scripts/dev ui
check` does. `cargo deny check` is a separate policy check, not part of the
gate; `cargo-deny` is not installed by the pinned toolchain.

Common focused commands:

```sh
cargo test -p daemon --test integration end_to_end_shell_session -- --exact
cargo test -p terminal-core --test golden plain_text -- --exact
cargo test -p client --lib ipc::tests::connect_request_and_event_round_trip -- --exact
scripts/dev ui check
```

Golden snapshots live in `crates/terminal-core/tests/snapshots/`; accepting a
new one needs `cargo-insta` installed separately.

## Running

```sh
scripts/dev daemon                 # isolated daemon in the foreground
scripts/dev daemon info            # the scratch socket/home/worktrees paths
scripts/dev ui run                 # Rune client on recorded fixture data
scripts/dev ui run --daemon        # connect to FORGE_SOCKET or the daily socket
scripts/dev ui --help              # every flag (--snap/--tree/--script/--bench)
```

`scripts/dev daemon` isolates itself: a fresh `$FORGE_DEV_DIR` gets
`FORGE_SOCKET` and `FORGE_HOME`, so it never contends with the daily daemon or
wipes its tabs, and its worktrees stay under `$FORGE_DEV_DIR/worktrees`. A
second daemon against the **same** lock exits successfully, so exit code 0 does
not prove a new daemon started. To inspect the daily daemon, run
`forge-daemon info` without those variables.

`scripts/dev ui` honours `CARGO_TARGET_DIR`; never point it at the root
`target/`. Rune's dependency build is large, so a shared target directory is
worth it when you build several checkouts.

### Editor

`scripts/dev ui run --open PATH` opens `PATH` in an editor session. It is
painted as lines when the daemon runs with `[editor] surface = "dom"`; with the
default `cells` it is a terminal. Name this checkout's build so the daemon and
editor control versions match:

```toml
[editor]
surface = "dom"
executable = "/path/to/forge-node/target/debug/forge-editor"
```

## Tests

- `crates/daemon/tests/integration.rs` starts its own daemon with an in-memory
  DB and a temporary socket. It needs writable `/tmp`, a real shell/PTY and
  real `git`; do not start a developer daemon for it.
- `git-service` integration tests return early when `git` is absent, so a green
  package test does not prove Git integration ran. Daemon E2E tests fail
  instead.
- Workspace tests are not fully hermetic: shell-environment coverage invokes
  the user's login shell, and daemon startup may run `--version` for installed
  agent CLIs.
- Ignored Phase 0 smokes exercise the real local agents (claude, opencode,
  cursor), `vim`/`htop` and latency. They are macOS reference checks, not a
  substitute for the deferred Wayland/X11 validation.
- The Rune client's pure tests live beside their modules; headless checks use
  `scripts/dev ui run --fixture --snap/--tree/--script`. `apps/ui/README.md`
  lists the scripts.

## Bundles

- `scripts/rune-app` builds `dist/rune/Forge Node Rune.app`: this checkout's
  client, daemon and editor, with its own data directory
  (`~/.forge-node-rune`). It never shares the everyday daemon's database.
- `scripts/package-macos` builds the release `dist/macos/Forge Node.app`
  (host arch, `--universal`, `--zip`, `--updater`), signs and notarizes when
  `FORGE_CODESIGN_IDENTITY` / `FORGE_NOTARY_PROFILE` are set, and emits the
  minisign-signed updater tarball. `scripts/package-linux` builds `.deb` /
  AppImage.
- `scripts/dist` adds cross-machine transfer (`build`, `ship`, `install`,
  `local`). It is a manual helper; release CI invokes `scripts/package-macos`
  directly.
- `scripts/release-manifest` writes `latest-rune.json`, the updater feed.

## Where to change what

| Need | Where |
|------|-------|
| Domain types, IDs, session state machine | `crates/domain` → [domain.md](./domain.md) |
| Wire requests/events/framing | `crates/protocol` → [protocol.md](./protocol.md) |
| PTY/engine/delta pipeline | `crates/daemon/src/terminal.rs`, `crates/terminal-core` → [terminal.md](./terminal.md) |
| Client replica and command methods | `crates/client` |
| Agent providers, detection, launch | `crates/agents` → [agents.md](./agents.md) |
| Git operations and diffs | `crates/git-service` |
| File reads/writes/search, definitions | `crates/fs-service` → [editor.md](./editor.md) |
| Editor engine and control wire | `crates/editor-*` → [editor.md](./editor.md) |
| SQLite schema and migrations | `crates/persistence` → [persistence.md](./persistence.md) |
| Rune shell, views, keymap, theme | `apps/ui/src` → [ui.md](./ui.md), [theming.md](./theming.md) |
| Release packaging and updater | `scripts/package-macos`, `scripts/rune-app`, `apps/ui/src/updates.rs` |
| Cost rules | [performance.md](./performance.md) |

## macOS capture and diagnostics

`scripts/dev ui run --snap PATH` writes a PNG of the rendered window;
`--tree PATH` writes the AccessKit tree; `--script FILE` drives keys, text,
drops and menu items on a virtual clock. Use an isolated `scripts/dev daemon`
for live checks; never the user's daily daemon or `/Applications`.
