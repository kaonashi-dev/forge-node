# Forge Node Tauri shell

The Forge GUI: Solid + Vite chrome in a WebView, over a Rust host that is a
workspace member and talks to `forge-daemon` through the existing `client`
crate. There is no second VT engine — the daemon owns the one there is, and this
side renders the cell replica it sends (ADR-011).

## Development

```sh
pnpm install
make run-tauri          # builds forge-daemon, then `pnpm tauri dev`
```

`FORGE_DAEMON_BIN` / `FORGE_SOCKET` override locator paths. The runtime thread
connects — or spawns the daemon — on launch, adds the current directory when no
project exists, and attaches a live shell.

## Checks

```sh
pnpm test               # vitest
pnpm build              # tsc --noEmit + vite build
pnpm lint               # oxlint (correctness, Solid-aware)
pnpm fmt:check          # oxfmt --check (printWidth 100, 2 spaces)
pnpm fmt                # oxfmt --write
cargo test -p forge-tauri
make check              # rust fmt/clippy/test + oxlint + vitest + tsc + cargo check
make lint               # oxlint only
make fmt-tauri          # oxfmt write
make fmt-check-tauri    # oxfmt check
```

After a protocol change, re-export the wire fixtures and regenerate the typed
copies the round-trip tests read; CI fails on drift between them:

```sh
cargo run -p protocol --bin export-fixtures
pnpm codegen
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
make package-linux      # .deb + AppImage  (scripts/package-linux, on Linux)
```

The daemon ships as a **sibling binary** next to the GUI rather than as a Tauri
sidecar: that is what `src-tauri/src/daemon/locator.rs` looks for, and it is
what lets `FORGE_DAEMON_BIN` point at a debug build.

## Layout

```
apps/tauri/
  src/
    actions/           chord → action dispatch
    harness/           feature types, gate rules, harness API
    palette/           command palette and its fuzzy filter
    panels/            right inspector: history, PR, features, lieutenant, files, git
    settings/          the settings route and its sections
    shell/             title bar, rail, tabs, status bar, attention bar
    store/             Solid stores: forge, runtime, workbench, harness, views
    terminal/          canvas renderer, viewport, selection, latency
    theme/             tokens, controls, icons
    ui/                shared control kit
    workbench/         centre tabs: editor, diff, feature, PR compose/detail
  src-tauri/           Rust host (runtime, workbench, cells, locator)
```
