# Forge Node

A native desktop app for working with **multiple terminals and multiple coding
agents** across one or more Git repositories, with first-class worktree support
and an architecture built from day one for a **graph of sessions** (a session can
spawn other sessions).

- **Terminal-first.** An agent is a terminal session running a known CLI
  (`claude`, `codex`, `opencode`, `cursor-agent`). New providers are just a
  descriptor.
- **The daemon owns execution.** Closing the GUI never kills your agents.
- **Organize related repositories.** Sidebar workspaces group projects, while
  ungrouped directories stay available under General.
- **Worktree = isolated workspace.** The main checkout is just another workspace.

## Install

Grab the newest build from
[Releases](https://github.com/kaonashi-dev/forge-node/releases/latest) —
`ForgeNode-<version>.zip`, macOS 11+ on Apple Silicon — and:

```sh
unzip ForgeNode-<version>.zip
xattr -dr com.apple.quarantine "Forge Node.app"
mv "Forge Node.app" /Applications/
```

The `xattr` line is needed once: these builds are ad-hoc signed rather than
signed with a Developer ID, so macOS quarantines what a browser downloaded.
From then on the app updates itself — it offers a new release in the status bar
and applies it on one click, without disturbing the sessions the daemon is
running. See [`docs/plan-updates.md`](./docs/plan-updates.md).

Start with [`docs/README.md`](./docs/README.md) (index of the implementation
docs: architecture, domain, protocol, terminal, agents, worktrees,
persistence, development). [`plan.md`](./plan.md) is the full target design
and [`execution.md`](./execution.md) the implementation log.

## Stack

Rust · Tauri 2 + Solid/Vite (GUI) · own daemon (runtime) · Unix domain socket + MessagePack (IPC) · Git CLI
· SQLite · `alacritty_terminal` (engine).

Target platforms: macOS (arm64 first) + Linux.

## Layout

```
crates/
  domain/         shared domain model + terminal wire types (the contract)
  protocol/       IPC framing, requests, responses, events, errors
  terminal-core/  PTY backend, TerminalEngine trait, alacritty engine
  terminal-input/ pure key/mouse/paste encoder shared with the GUI
  agents/         provider registry, detection, built-ins
  git-service/    git CLI wrapper, repository + worktree ops
  persistence/    SQLite schema, migrations, repositories
  client/         protocol client + cell-grid replica
  daemon/         the runtime: services, PTYs, sessions, IPC  → binary `forge-daemon`
  test-support/   fake PTY / fake agent / temp repo helpers
apps/tauri/       Tauri 2 + Solid shell; Rust host is under `src-tauri/`
```

Dependency direction: `forge-tauri → client → {protocol, terminal-input} → domain`;
`daemon → {agents, git-service, persistence, terminal-core} → domain`.

## Build

```sh
cargo build --workspace
cargo test  --workspace
```

Requires Rust 1.89 (pinned in `rust-toolchain.toml`), `git`, and a C toolchain
(for bundled SQLite).

## Run

```sh
make dev                          # daemon + Tauri Solid shell (`apps/tauri`)
make build-tauri                  # Tauri frontend + Rust host
make test-tauri                   # vitest + cargo test -p forge-tauri
scripts/dev daemon                # cargo run -p daemon --bin forge-daemon
forge-daemon info                 # resolved socket / db / worktrees / log paths + config
forge-daemon run                  # take the singleton lock, bind the socket, serve
scripts/dev check                 # fmt --check + clippy -D warnings + test (the CI gate)
cargo deny check                  # license & advisory policy
```

The daemon is a per-user singleton (advisory `flock`); a second `run` exits 0.
See [`docs/development.md`](./docs/development.md) for paths and testing notes.

## Package

Never run automatically — neither by `scripts/dev` nor by CI. Run by hand on
the matching platform:

```sh
scripts/package-macos              # dist/macos/Forge Node.app (arm64)
scripts/package-macos --universal  # arm64 + x86_64 via lipo
scripts/package-linux deb          # dist/linux/forge_<ver>_<arch>.deb
scripts/package-linux appimage     # dist/linux/ForgeNode-<ver>-<arch>.AppImage
```

Both bundles keep `forge-tauri` and `forge-daemon` **side by side**, because the GUI
finds the runtime with `current_exe().with_file_name("forge-daemon")`.

Code signing and notarization on macOS are optional and driven entirely by
environment variables (`FORGE_CODESIGN_IDENTITY`, `FORGE_ENTITLEMENTS`,
`FORGE_NOTARY_PROFILE`); with none set you get an unsigned local bundle.
`scripts/package-* --help` prints the full contract.

## Distribute

To build on one machine and run on another, `scripts/dist` wraps the packagers
into a single self-contained tarball:

```sh
scripts/dist build                       # dist/release/forge-<ver>-<os>-<arch>.tar.gz
scripts/dist ship <user@host>            # build + scp + print the remote install line
# on the target machine, no checkout needed:
tar xzf forge-<ver>-<os>-<arch>.tar.gz && ./forge-<ver>-<os>-<arch>/install.sh
```

The embedded `install.sh` strips the quarantine attribute, checks the bundle's
architecture against the host, replaces any previous Forge Node.app in
`/Applications` (falling back to `~/Applications`) and verifies the signature
when one is present. Build with `--universal` when the two Macs have different
CPUs.

`cargo deny check licenses` must pass before distributing anything (ADR-002).

## Status

MVP in progress; product name `Forge Node` (binaries `forge-tauri` / `forge-daemon`).

**Implemented and tested** (`scripts/dev check`): the whole backend —
`domain`, `protocol` (framing, handshake, ~30
requests, events, errors), `terminal-core` (PTY, alacritty engine, deltas),
`terminal-input` (pure input mapping), `agents` (registry, verified detection,
four built-ins), `git-service`
(repo + worktree operations), `persistence` (SQLite, migrations, orphan
reconciliation), `client` (sync protocol client + cell-grid replica) and
`daemon` (services, PTY loop, sessions, IPC), including an end-to-end test that
drives a real daemon over a real socket with a real shell.

The Tauri shell (`apps/tauri`) is the GUI. It connects or starts the daemon and
renders a passive `client::CellGrid`.

`forge-daemon dump`/`stats` are not wired.

Track progress in [`execution.md`](./execution.md).

## License

MIT OR Apache-2.0.
