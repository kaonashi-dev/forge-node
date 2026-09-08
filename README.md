<h1 align="center">Forge Node</h1>

<p align="center">
  Run <b>Claude Code, Codex, OpenCode and Cursor side by side</b> — each in its own
  git worktree, every terminal owned by a daemon that outlives the window.
</p>

<p align="center">
  <a href="https://github.com/kaonashi-dev/forge-node/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/kaonashi-dev/forge-node?color=2f81f7"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20arm64-lightgrey">
  <img alt="License" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue">
</p>

<p align="center">
  <b><a href="https://github.com/kaonashi-dev/forge-node/releases/latest/download/ForgeNode-macos-arm64.zip">Download for macOS (Apple Silicon)</a></b>
</p>

## Install

macOS 11 or newer, Apple Silicon. `git` on `PATH`.

```sh
unzip ForgeNode-macos-arm64.zip
xattr -dr com.apple.quarantine "Forge Node.app"
mv "Forge Node.app" /Applications/
open "/Applications/Forge Node.app"
```

**The `xattr` line is not optional.** These builds are ad-hoc signed rather than
signed with a Developer ID, so macOS quarantines anything a browser downloaded
and refuses to open it — the app appears "damaged" until the attribute is
cleared. You pay this once, on the first install.

Every other build after that arrives on its own: the app checks for a release
shortly after launch and every six hours, offers it as a pill in the status bar,
and applies it on one click. The runtime is a separate process that an update
never touches, so your sessions, terminals and scrollback survive the reload.
See [`docs/plan-updates.md`](./docs/plan-updates.md) for how that works and what
it refuses to do in place.

Intel Macs and Linux are not published yet — build them from source with
[`scripts/package-macos --universal`](#packaging) or `scripts/package-linux`.

## Why

- **Terminal-first.** An agent is a terminal session running a known CLI. There
  is no bespoke integration per provider — a new one is a descriptor.
- **The daemon owns execution.** Closing the GUI never kills your agents. The
  window is a passive replica of a grid the daemon renders.
- **Worktree = isolated workspace.** The main checkout is just another
  workspace, and a new one is provisioned from per-project share rules so it
  starts with the `.env` and friends a fresh `git worktree add` leaves behind.
- **A graph of sessions.** A session can spawn other sessions, and the rail
  indents the children under the orchestrator that owns them.
- **Reads that stay reads.** Diffs, file tree, search, pull requests and a
  read-only review mode for someone else's PR, all answered by the daemon —
  the GUI never touches a working tree itself.

## Supported agents

| Agent | CLI | Own config dir | Usage & spend |
| --- | --- | --- | --- |
| Claude Code | `claude` | `CLAUDE_CONFIG_DIR` | remaining allowance + token analytics |
| Codex CLI | `codex` | `CODEX_HOME` | remaining allowance + token analytics |
| OpenCode | `opencode` | `OPENCODE_CONFIG_DIR` | — |
| Cursor CLI | `cursor-agent` | — | — |

A launch profile is the saved form of a shell wrapper — its own config
directory, arguments and environment — so the same agent can run twice against
two accounts. Cursor documents no directory of its own, so a profile for it can
change the binary and the arguments and nothing else.

Detection is verified, not assumed: each candidate binary is version-probed
before it is offered. Providers live in `crates/agents`; nothing else in the
workspace is allowed to branch on a provider id.

## Documentation

[`docs/README.md`](./docs/README.md) indexes the implementation docs —
architecture, domain, protocol, terminal, agents, worktrees, persistence,
development. [`plan.md`](./plan.md) is the target design and
[`execution.md`](./execution.md) the implementation log; both can contradict the
code, which is why [`AGENTS.md`](./AGENTS.md) is the invariant list that cannot.

## Developing

Rust 1.89 (pinned in `rust-toolchain.toml`), `git`, and a C toolchain for
bundled SQLite.

```sh
make dev             # daemon + Tauri/Solid shell, hot reload
scripts/dev check    # fmt --check + clippy -D warnings + test — the CI gate
make test-tauri      # vitest + cargo test -p forge-tauri
scripts/dev daemon   # run the runtime alone
forge-daemon info    # resolved socket / db / worktrees / log paths + config
```

The daemon is a per-user singleton behind an advisory `flock`; a second `run`
exits 0 rather than failing, so a zero exit code does not prove a new one
started. [`docs/development.md`](./docs/development.md) has the paths and the
testing traps.

### Layout

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
  daemon/         the runtime: services, PTYs, sessions, IPC → `forge-daemon`
apps/tauri/       Tauri 2 + Solid shell; the Rust host is under `src-tauri/`
```

Dependency direction: `forge-tauri → client → {protocol, terminal-input} → domain`
and `daemon → {agents, git-service, persistence, terminal-core} → domain`.

Rust · Tauri 2 + Solid/Vite · Unix domain socket + MessagePack · Git CLI ·
SQLite · `alacritty_terminal`.

### Packaging

Never run automatically — not by `scripts/dev`, not by CI. Run by hand on the
matching platform:

```sh
scripts/package-macos --updater --zip   # dist/macos/Forge Node.app + release assets
scripts/package-macos --universal       # arm64 + x86_64 via lipo
scripts/package-linux deb               # dist/linux/forge_<ver>_<arch>.deb
scripts/package-linux appimage          # dist/linux/ForgeNode-<ver>-<arch>.AppImage
```

Every bundle keeps `forge-tauri` and `forge-daemon` **side by side**: the GUI
finds the runtime with `current_exe().with_file_name("forge-daemon")`. Signing
and notarization are optional and driven entirely by `FORGE_CODESIGN_IDENTITY`,
`FORGE_ENTITLEMENTS` and `FORGE_NOTARY_PROFILE`; with none set you get an
unsigned local bundle. `scripts/package-* --help` prints the full contract.

`scripts/dist` wraps the packagers into one self-contained tarball with an
embedded `install.sh`, for copying a build to a machine that has no checkout:

```sh
scripts/dist build                  # dist/release/forge-<ver>-<os>-<arch>.tar.gz
scripts/dist ship <user@host>       # build + scp + print the remote install line
```

`cargo deny check licenses` must pass before anything is distributed (ADR-002).

### Releasing

The version has one source — `[workspace.package]` in `Cargo.toml`. Bump it,
tag it, push the tag; `.github/workflows/release.yml` runs the gate, builds and
signs the bundle, writes the updater manifest and publishes the release.

```sh
git commit -am "Release 0.2.0"
git tag -a v0.2.0 -m "What changed."
git push origin main v0.2.0
```

Never delete a tag that already has a release: GitHub demotes the release to a
draft, and the updater endpoint silently falls back to the previous version.

## Status

MVP in progress. The backend is implemented and covered by `scripts/dev check`:
`domain`, `protocol` (framing, handshake, ~30 requests, events, errors),
`terminal-core` (PTY, alacritty engine, deltas), `terminal-input`, `agents`
(registry, verified detection, four built-ins), `git-service`, `persistence`
(SQLite, migrations, orphan reconciliation), `client` and `daemon` — including
end-to-end tests that drive a real daemon over a real socket with a real shell.

`forge-daemon dump` is the one command still unwired. Track progress in
[`execution.md`](./execution.md).

## License

MIT OR Apache-2.0.
