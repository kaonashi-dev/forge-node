<h1 align="center">Forge Node</h1>

<p align="center">
  <b>A native desktop workbench for terminals and AI coding agents.</b><br>
  Run Claude Code, Codex, OpenCode, Cursor and Grok across Git repositories and
  isolated worktrees — with sessions that outlive the window.
</p>

<p align="center">
  <a href="https://github.com/kaonashi-dev/forge-node/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/kaonashi-dev/forge-node?color=2f81f7"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20arm64-lightgrey">
  <img alt="License" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue">
</p>

<p align="center">
  <b><a href="https://github.com/kaonashi-dev/forge-node/releases/latest/download/ForgeNode-macos-arm64.zip">Download for macOS (Apple Silicon)</a></b>
</p>

<p align="center">
  <a href="#features">Features</a> ·
  <a href="#screenshots">Screenshots</a> ·
  <a href="#install">Install</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#development">Development</a> ·
  <a href="#documentation">Documentation</a>
</p>

> [!NOTE]
> **A personal, experimental project.** Built for the way I work and published
> in case it is useful to someone else — not a product. There is no support and
> no roadmap, the surface moves without deprecation periods, and a release can
> break what the last one did. Read it, fork it, take ideas from it; depend on
> it only with that in mind.

## Features

- **Your agents, their own CLIs.** Launch Claude Code, Codex, OpenCode, Cursor
  CLI and Grok in real terminals, using their existing authentication and
  configuration. Forge runs their interactive interfaces, not a replacement
  chat UI.
- **Isolated workspaces.** Give parallel tasks separate Git worktrees and
  branches. Per-project share rules can copy, clone or link files such as `.env`
  and dependencies, or run a setup command in a new worktree.
- **Long-lived terminals.** Close and reopen the window without stopping your
  shells or agents. Split the view to keep two terminals, or a terminal and a
  file, visible together.
- **Code beside the conversation.** Browse files, edit with the integrated
  `forge-editor`, search the workspace, jump to definitions, and preview
  Markdown, SVG and images.
- **Git and pull requests.** Inspect highlighted diffs, compare branches,
  resolve conflicts, draft commits, and open or review GitHub pull requests.
  Agent PR reviews use the provider's read-only mode.
- **Profiles and usage.** Save launch arguments and separate accounts, choose
  a default agent, and view supported providers' allowance meters and token
  analytics.
- **Sessions that cooperate.** Hand off context, spawn child sessions across
  providers, and coordinate runs and tasks through `forgectl`.

## Screenshots

**Code view** — a workspace file tree and integrated Rust editor, with shell
and agent sessions available in the tab bar.

<img width="1200" alt="Forge Node Code view showing a file tree, Rust editor, and terminal and agent tabs" src="https://github.com/user-attachments/assets/dcdc511f-9587-438f-ba4e-17954a58f1f3" />

<details>
<summary>More screenshots: agent launcher and settings</summary>

**Agent launcher** — start a terminal, a detected agent CLI, or a saved launch
profile from the **+** menu. Custom profile names are not additional built-in
providers.

<img width="1200" alt="Claude Code running in Forge Node with the terminal, agent and launch profile menu open" src="https://github.com/user-attachments/assets/39639740-45db-4429-9bde-e4e38428e0ba" />

**Agent settings** — choose your default agent, inspect detected CLI versions,
enable providers, and manage launch profiles.

<img width="1200" alt="Forge Node Agents settings showing the default agent selector, five detected providers and a custom OpenCode launch profile" src="https://github.com/user-attachments/assets/993c0dfd-a6d7-4ddd-81da-6e2e848dc5fa" />

</details>

## Install

Published builds target **macOS 11 or newer on Apple Silicon**.

- Have `git` available on `PATH`.
- Install and authenticate the agent CLIs you want to use; they are not bundled.
  Plain shell sessions work without an agent installed.
- For GitHub pull-request features, install the [GitHub CLI](https://cli.github.com/)
  and sign in with `gh auth login`.

Download [the latest macOS ZIP](https://github.com/kaonashi-dev/forge-node/releases/latest/download/ForgeNode-macos-arm64.zip),
then run these commands from the directory containing it:

```sh
unzip ForgeNode-macos-arm64.zip
xattr -dr com.apple.quarantine "Forge Node.app"
mv "Forge Node.app" /Applications/
open "/Applications/Forge Node.app"
```

> [!IMPORTANT]
> Published builds are ad-hoc signed rather than Developer ID–signed. macOS
> may report that the app is "damaged" until the `xattr` command clears its
> quarantine attribute. Only do this for a release you downloaded from this
> repository and trust.

### Updates and other platforms

The app checks for updates shortly after launch and every six hours. Use
**Check for Updates** in the app menu to check manually. Protocol-compatible
updates relaunch the GUI while the daemon, sessions and scrollback keep running.
A protocol-changing release requires a manual install and a deliberate daemon
restart; restarting the daemon ends its live processes.

Intel macOS and Linux builds are not published. Source packagers are available
in [Packaging](#packaging), but Linux Wayland/X11 runtime validation remains
deferred. Windows is not a target.

## Quick start

1. Open the command palette with **Cmd+K**, choose **Add Project…**, and select
   a repository. Existing worktrees are discovered alongside the main checkout.
   Plain directories can also be projects, without Git worktree actions.
2. In **Settings → Agents**, check detection and choose a default agent. Use
   **Refresh** after installing a CLI, or configure its executable if needed.
3. Select a workspace, then use the **+** menu to start a terminal or agent.
   **Cmd+Shift+A** launches your default agent, or opens the picker if you chose
   **Ask each time**.
4. For an independent task, create a worktree with **Cmd+Shift+N**. Configure
   **Settings → Shared files** if the project needs untracked files or setup
   commands in new worktrees.
5. Open **Files**, **Git**, **History** or **PR** in the sidebar to move between
   code, changes, previous agent sessions and pull requests.

### Useful macOS shortcuts

| Action | Shortcut |
| --- | --- |
| Command palette | **Cmd+K** |
| New terminal | **Cmd+T** |
| Default agent / agent picker | **Cmd+Shift+A** |
| New worktree | **Cmd+Shift+N** |
| Find a file | **Cmd+P** |
| Search workspace contents | **Cmd+Shift+F** |
| Split terminal or editor view | **Cmd+D** |
| Settings | **Cmd+,** |

Bindings are customizable in **Settings → Keyboard**. See [UI](docs/ui.md)
for the full layout and platform-specific behavior.

## Supported agents

| Agent | CLI candidates | Separate account directory | Usage in Forge |
| --- | --- | --- | --- |
| Claude Code | `claude` | `CLAUDE_CONFIG_DIR` | Allowance + token analytics |
| Codex CLI | `codex` | `CODEX_HOME` | Allowance + token analytics |
| OpenCode | `opencode`, `opencode2` | `OPENCODE_CONFIG_DIR` + `XDG_DATA_HOME` | — |
| Cursor CLI | `agent`, `cursor-agent` | Not supported | — |
| Grok | `grok` | `GROK_HOME` | Allowance |

A **launch profile** saves a provider's executable, account directory and
arguments. Use profiles to run the same provider with different accounts or
models. Cursor profiles can change the executable and arguments, but not the
account directory. Profiles configure existing providers; custom provider
definitions in `config.toml` are not implemented.

Every candidate executable is version-probed before it is offered. Ambiguous
names such as `agent` are checked for the expected provider. Provider details,
resume support, read-only modes and usage sources: [Agent providers](docs/agents.md).

## Configuration and session lifetime

Use Settings for agent profiles, keyboard bindings, shared files and appearance.
Optional daemon settings live in `config.toml`; a missing file uses defaults,
and file changes require a daemon restart. See the annotated
[configuration example](docs/config.example.toml) for paths and options.

**Closing the window is not the same as stopping the daemon.** Live sessions
and bounded in-memory scrollback survive a GUI close or compatible update,
not a daemon restart or reboot. SQLite stores metadata, not terminal output.
By default, session rows are cleared on daemon startup;
`sessions.persist_history = true` retains them as orphaned, restartable history,
not as running processes. Projects and workspaces survive either way.

## How it works

```text
forge-tauri (Tauri + Solid) ── Unix socket / MessagePack ── forge-daemon
  window, controls, passive cell grid                       PTYs, VT engine,
                                                           sessions, Git, SQLite
forgectl ──────────────────────────────────────────────────► runs, tasks, context
```

The daemon owns every session process and the only terminal emulator
(`alacritty_terminal`). The GUI renders snapshots and damaged-row deltas; it
does not run a second emulator or access workspace files directly. Git and
file operations go through daemon services. `forgectl` is a client of the same
runtime, not a separate job runner.

See [Architecture](docs/architecture.md) for the crate graph and runtime boundaries.

## Development

### Prerequisites

- Rust **1.89.0**, pinned in [`rust-toolchain.toml`](rust-toolchain.toml).
- [Bun](https://bun.sh), **latest** with a **1.4.3** minimum.
- `git`, `make`, and a C toolchain for bundled SQLite.
- macOS: Xcode Command Line Tools (`xcode-select --install`).
- Linux: Tauri's [system prerequisites](https://v2.tauri.app/start/prerequisites/#linux).

### Run from source

```sh
git clone https://github.com/kaonashi-dev/forge-node.git
cd forge-node
make install-ui
make build-rust
make dev
```

The initial workspace build includes `forge-editor` and `forgectl` alongside
the daemon. For subsequent iterations, `make dev` builds the debug daemon and
launches the Tauri + Solid shell with hot reload. By default, the GUI connects
to the daily per-user daemon. To isolate development, start `scripts/dev daemon`,
then apply the environment exports it prints in a second terminal before running
`make dev` there.

### Checks and builds

| Command | Purpose |
| --- | --- |
| `scripts/dev check` | Canonical Rust gate: format check, Clippy with warnings denied, workspace tests |
| `make check` | Rust gate plus frontend lint, format, tests, typechecks, boundary checks and bundle budgets |
| `make test-tauri` | Frontend tests and typechecks plus Rust host tests |
| `cargo build --workspace` | Build all Rust workspace members |
| `make install-local` | macOS release build, install in Applications, and relaunch the GUI |
| `scripts/dev daemon` | Run an isolated development daemon |
| `forge-daemon info` | Print resolved runtime paths and effective configuration |

CI runs the Rust and frontend gates on macOS. `cargo deny check` is a separate
dependency policy check, not part of those gates. `forge-daemon dump` remains
an unwired placeholder; `forge-daemon stats` reads a running daemon and does
not start one.

Development paths, SDK selection, exact test commands and singleton behavior:
[Development guide](docs/development.md). Read [AGENTS.md](AGENTS.md) before
contributing, and [Performance](docs/performance.md) before changing terminal,
rendering, locking or subprocess paths.

### Packaging

Run each packager on its matching platform. Packaging is separate from the
development gates and ordinary PR CI.

```sh
scripts/package-macos --updater --zip   # dist/macos/Forge Node.app + release assets
scripts/package-macos --universal       # arm64 + x86_64 via lipo
scripts/package-linux deb               # dist/linux/forge_<ver>_<arch>.deb
scripts/package-linux appimage          # dist/linux/ForgeNode-<ver>-<arch>.AppImage
```

Bundles include the GUI, `forge-daemon`, `forge-editor` and `forgectl` as sibling
binaries. Local Developer ID signing and notarization are optional, configured
with `FORGE_CODESIGN_IDENTITY`, `FORGE_ENTITLEMENTS` and `FORGE_NOTARY_PROFILE`.
Updater artifacts also require a Tauri signing key. Run the packager with
`--help` for its requirements.

For a machine without a checkout, `scripts/dist` creates a self-contained
tarball with an embedded installer:

```sh
scripts/dist build                  # dist/release/forge-<ver>-<os>-<arch>.tar.gz
scripts/dist ship user@host          # build, copy with scp, print install command
```

`make install-local` leaves the daemon running so active terminals survive;
a protocol-changing build still needs a deliberate daemon restart.

`cargo deny check licenses` must pass locally before anything is distributed
([ADR-002](docs/decisions.md)). Install `cargo-deny` separately; it is not
included in the pinned Rust toolchain.

### Releasing

Set the version in `[workspace.package]` in `Cargo.toml` and keep
`apps/tauri/package.json` in sync. Push a matching `v<version>` tag to trigger
the [release workflow](.github/workflows/release.yml), which validates the tag
against Cargo, runs the Rust gate, builds the macOS bundle and signed updater
artifact, and publishes the release and updater manifest.

Never delete a tag with a published release: GitHub demotes that release to a
draft, and the updater can fall back to the previous version.

## Documentation

Start with the [documentation index](docs/README.md) or choose a topic:

| Topic | Guide |
| --- | --- |
| Vocabulary and project map | [CONTEXT.md](CONTEXT.md) |
| Architecture and crate layout | [Architecture](docs/architecture.md) |
| Agents, profiles and usage | [Agent providers](docs/agents.md) |
| Worktrees and provisioning | [Worktrees](docs/worktrees.md) |
| Editing and previews | [Editor](docs/editor.md) |
| Layout and keyboard behavior | [UI](docs/ui.md) |
| Session handoffs and child sessions | [Session context](docs/session-context.md) |
| Runs, tasks and the CLI | [Orchestration](docs/orchestration.md) |
| Runtime configuration | [Configuration example](docs/config.example.toml) |
| Contributor setup and tests | [Development](docs/development.md) |
| Design decisions and invariants | [Decisions](docs/decisions.md) · [AGENTS.md](AGENTS.md) |

These guides describe implemented behavior. Working plans and historical
checkpoints stay in a local, unpublished `plan/` directory.

## License

MIT OR Apache-2.0.
