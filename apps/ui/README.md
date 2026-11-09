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

## Boundaries

The rules in `AGENTS.md` apply unchanged. The daemon owns the VT engine. This app keeps the
passive `client::CellGrid` replica, and `client::Client` runs on a worker thread, never on
the UI thread. Terminal deltas are applied in order and published latest-wins. The
`client::RowDamage` of skipped publications is merged, never dropped.
