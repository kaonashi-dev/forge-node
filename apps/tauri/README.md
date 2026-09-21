# Forge Node Tauri shell

The Forge GUI: Solid + Vite chrome in a WebView, over a Rust host that is a
workspace member and talks to `forge-daemon` through the existing `client`
crate. There is no second VT engine — the daemon owns the one there is, and this
side renders the cell replica it sends (ADR-011).

## Development

```sh
bun install
make run-tauri          # builds forge-daemon, then `bun run tauri dev`
```

`FORGE_DAEMON_BIN` / `FORGE_SOCKET` override locator paths. The runtime thread
connects — or spawns the daemon — on launch, adds the current directory when no
project exists, and attaches a live shell.

## Checks

```sh
bun run check              # canonical: lint + fmt + vitest + bun:test + boundaries + build
bun run test               # Vitest + Bun tooling/package tests
bun run build              # typecheck + vite build + bundle budgets
bun run boundaries         # detected ownership edges and import cycles (limitations in architecture doc)
bun run lint               # oxlint (correctness, Solid-aware)
bun run fmt:check          # oxfmt --check (printWidth 100, 2 spaces)
bun run fmt                # oxfmt --write
cargo test -p forge-tauri
```

After a protocol change, re-export the wire fixtures and regenerate the typed
copies the round-trip tests read; CI fails on drift between them.

The generator writes `src/contracts/generated/fixtures.ts`; wire type mirrors
live alongside it under `src/contracts/` and are maintained separately:

```sh
cargo run -p protocol --bin export-fixtures
bun run codegen
```

Two measurements have their own entry points:

```sh
make latency-tauri      # key-to-render p95 (needs a live daemon); budget 50 ms
cargo run -p forge-tauri --bin forge-tauri-probe   # first delta sequence
```

For paint cost, set `localStorage.forgeTerminalDebug = "1"` in the WebView: the
overlay's first line is the round trip, the second is `paint p50/p95/max` with
the cell count, marked `OVER` past the 8 ms budget.

## Packaging

```sh
make package            # macOS Forge.app  (scripts/package-macos)
make install-local      # release build, replace the installed app, relaunch it
make package-linux      # .deb + AppImage  (scripts/package-linux, on Linux)
```

The daemon ships as a **sibling binary** next to the GUI rather than as a Tauri
sidecar: that is what `src-tauri/src/daemon/locator.rs` looks for, and it is
what lets `FORGE_DAEMON_BIN` point at a debug build.

## Layout

```
apps/tauri/
  src/
    app/               composition: lifecycle, shell, integrations, palette
    contracts/         wire types (generated/ holds generated fixtures)
    runtime/           Tauri invoke transport, frame channels, shared clock
    state/             snapshot, connection, workspace, preferences, dialogs
    navigation/        parked views, tab order/MRU, sidebar state
    actions/           chord → action dispatch
    features/          terminal, editor, files, git, pull-requests, sessions,
                       projects, settings
    shared/            cell-grid, input, markdown, paths
    theme/             tokens, theme primitives, icons
    ui/                shared control kit
  src-tauri/src/
    lib.rs             Tauri setup and command registration
    commands.rs        thin Tauri command entry points
    menus.rs           native menu construction
    daemon/            daemon connection and startup
    runtime/           runtime loop, input mapping, cell encoder, snapshots
      workbench/       one bounded worker; command definitions and capability handlers
```

Ownership, dependency rules and "where to change X": [docs/frontend-architecture.md](../../docs/frontend-architecture.md).
