# forge-ui

The Rune client for Forge Node. It is built and run beside `apps/tauri` while it reaches
parity. The Tauri app is still the one that ships.

It is its own Cargo workspace on Rust 1.99.1 (`rust-toolchain.toml`), because Rune needs
edition 2024 and the root workspace stays on 1.89.0. `scripts/dev check` does not build it.

## Commands

```sh
scripts/dev ui check                  # fmt-check + clippy -D warnings + tests
scripts/dev ui run                    # recorded snapshot (fixtures/snapshot.json), no daemon
scripts/dev ui run --daemon           # the daemon FORGE_SOCKET (or the default socket) names
scripts/dev ui run --daemon --spawn-daemon --daemon-bin target/debug/forge-daemon
scripts/dev ui run --bench            # headless session-switch + echo timings (fixture)
scripts/dev ui run --help             # every flag, including Rune's --snap/--tree/--script
```

`scripts/dev ui` honours `CARGO_TARGET_DIR`. Rune's dependency build is large, so point
it at one shared directory if you build several checkouts, never at the root `target/`.

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
