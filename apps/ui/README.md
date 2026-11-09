# forge-ui

The Rune client for Forge Node, and the app that ships.

It is its own Cargo workspace on Rust 1.99.1 (`rust-toolchain.toml`), because Rune needs
edition 2024 and the root workspace stays on 1.89.0. `scripts/dev check` runs the root
gate and then this one.

## Parity

Done: the rail (groups, folds, keyboard walk, "Needs you", projects, checkouts and their
order, sessions, add project, new worktree from a branch picker, remove worktree with the
daemon's refusal and a force, action chips and the ▶ action menu), the actions editor,
shared files (Settings and per checkout), ignored worktrees, terminal and agent sessions,
the tab strip and sidebar views, the GPU terminal (split column, ⌘-links, click to move
the cursor, zoom, bell attention, file drops), the daemon editor surface (find panel, go
to definition and back, the conflict note with Compare, type size, line spacing,
autosave, reopen closed and the pane menu), a live file tree (watched folders, git
decorations, filter, following the active editor) with Markdown, CSV, image and code
previews, project search (⇧⌘F), quick open with recent files, the Git panel (diff,
commit, Juva draft, fetch, stopped rebase, branch compare, conflict blocks and a split
layout), pull requests (scope filters, detail, review with an agent, agent-composed
draft), session workflow (handoff, spawn child, send context, session changes,
structured review), History, Processes, palette scopes (`>` `/` `@` `#`), Ctrl-Tab MRU,
tab drag order, a rebindable keymap, toasts, Settings (theme, appearance controls,
agents and profiles, keyboard, shared files, Storage with legacy-home migration, Stats),
native menus, About, the signed updater, and the context menus of projects, checkouts,
sessions, tabs, the editor pane and the file tree (with its create, rename, duplicate,
move and delete).

Known gaps, none of which block the shipping app: the compare view renders plain patch
rows rather than syntax-coloured ones; ⌘D splits terminals, not an open file; the
`/` and `#` palette scopes answer from the daemon, so the recorded fixture cannot show
results; Glass uses translucent chrome tokens because Rune's window is opaque; and a
protocol-breaking update still needs a manual install.

## Commands

```sh
scripts/dev ui check                  # fmt-check + clippy -D warnings + tests
scripts/dev ui run                    # recorded snapshot (fixtures/snapshot.json), no daemon
scripts/dev ui run --daemon           # the daemon FORGE_SOCKET (or the default socket) names
scripts/dev ui run --daemon --spawn-daemon --daemon-bin target/debug/forge-daemon
scripts/dev ui run --bench            # headless session-switch + echo timings (fixture)
scripts/dev ui run --help             # every flag, including Rune's --snap/--tree/--script
```

### Editor

`scripts/dev ui run --open PATH` opens `PATH` (relative to the first workspace) in an
editor session. It is painted as lines only when the daemon runs with
`[editor] surface = "dom"`; with the default `cells` it is a terminal like any other.
`scripts/dev daemon` finds `forge-editor` on the login shell's `PATH` before the one it
was built with, which is the installed app's. Name this checkout's build so the control
versions match:

```toml
[editor]
surface = "dom"
executable = "/path/to/forge-node/target/debug/forge-editor"
```

`scripts/dev ui` honours `CARGO_TARGET_DIR`. Rune's dependency build is large, so point
it at one shared directory if you build several checkouts, never at the root `target/`.

## Performance harness

```sh
scripts/dev ui run --bench all     # fixture switch/echo + 125 Hz terminal + editor JSON
scripts/dev ui run --bench all --bench-report /tmp/perf.json
apps/ui/scripts/perf.sh             # everything: adds startup/RSS and the Tauri baseline
```

`--bench all` runs the session-switch/echo fixture, the integrated 125 Hz terminal stream
(128×49, rich full scroll, one `SharedTerminal::apply_delta` per frame through the real
compositor on a headless GPU) and the 88-row `domain::EditorFrame` fixture. `--bench
terminal-125` and `--bench editor` run one section; `--bench-iterations` and `--bench-frames`
size them.

`scripts/perf.sh` writes `scripts/perf-report.json` and `scripts/perf-report.md`. It also
measures `forge-ui` startup to the first drawn frame and RSS at 0/1/2 s after that frame, and
runs the Tauri `forge-tauri-latency` route against a temporary daemon. The Tauri section needs
prebuilt `forge-daemon` and `forge-tauri-latency` binaries; point `PERF_TAURI_ROOT` at a
checkout that has them (`make latency-tauri` builds them in the root workspace). It never
points the latency tool at the user's live daemon.

The terminal build/encode clocks come from the compositor's own `Stats`; they are CPU time,
not GPU time. Editor key-to-paint is a CPU proxy (key handler through one 125 Hz frame) unless
a display/vsync measurement is added later.

## Input gate and focus

Keys, paste and IME commits are held while a terminal is `Attaching` and replayed once, in
order, after both a published snapshot and a compositor measurement exist. The hold is
capped at 64 KiB; a gesture that would cross it is refused whole and shown in a
`terminal-hold-notice` banner. Disconnect, a terminal switch or a generation change drops
the queue, and a resync waits for a measurement rendered after it.

Sidebar rows are select-only and the shell is wrapped in a `terminal-focus-proxy`, so a row
click moves real focus to the proxy and its key handler is the terminal route. The
`terminal-focus-indicator` names the active pane. `scripts/focus-session.json` proves a row
click followed by typing, and `scripts/check_tree.py` asserts both a11y nodes.

## Boundaries

The rules in `AGENTS.md` apply unchanged. The daemon owns the VT engine. This app keeps the
passive `client::CellGrid` replica, and `client::Client` runs on a worker thread, never on
the UI thread. Terminal deltas are applied in order and published latest-wins. The
`client::RowDamage` of skipped publications is merged, never dropped.
