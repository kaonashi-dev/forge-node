# Tauri UI — progress register

Status for the Tauri + Solid shell.

| Document                 | Role               |
| ------------------------ | ------------------ |
| [PARITY.md](./PARITY.md) | Feature checklist  |
| [README.md](./README.md) | Build, run, layout |

**Last updated:** 2026-08-30

---

## Summary

| Metric                 | Value                                                     |
| ---------------------- | --------------------------------------------------------- |
| **Current phase**      | 6 (polish)                                                |
| **Phases complete**    | 0–5: scaffold, wire, terminal, chrome, workbench, harness |
| **Phases in progress** | 6 (polish), 7 (ship)                                      |
| **GUI**                | **Tauri** (`make run`, `make run-tauri`)                  |

## Phase status

| Phase | Name                     | Status | Gate / notes                                                                                                              |
| ----- | ------------------------ | ------ | ------------------------------------------------------------------------------------------------------------------------- |
| **0** | Scaffold & design system | ✅     | Themed 3-column window; tokens + mix(); CI builds and typechecks                                                          |
| **1** | Wire layer               | ✅     | Runtime thread + probe; `runtime:cells` split off `runtime:state`                                                         |
| **2** | Terminal                 | ✅     | Canvas renderer, input, resize, scrollback, selection; p95 10.5 ms                                                        |
| **3** | Navigation chrome        | ✅     | Actions + palette + rail tree + settings route + native menus                                                             |
| **4** | Workbench                | ✅     | Diff/editor/rebase/PR compose; projects, groups, agent profiles                                                           |
| **5** | Harness                  | ✅     | Features panel, feature tab, job stream, gate bar, preview terminal                                                       |
| **6** | Polish                   | 🚧     | Mouse reporting, control states, ARIA tree, disconnect banner, token parity checked in CI; the paint number is unrecorded |
| **7** | Ship                     | 🚧     | macOS + Linux bundles and the two-GUI docs; `externalBin` deliberately not used                                           |

## Gates

```sh
make run-tauri                 # daemon sibling + pnpm tauri dev
make check                     # fmt + clippy + rust tests + vitest + tsc + cargo check
make latency-tauri             # Phase 2 gate: key-to-render p95 <= 50 ms
make package                   # macOS .app          (Linux: make package-linux)
cargo run -p forge-tauri --bin forge-tauri-probe
pnpm --dir apps/tauri codegen   # after export-fixtures
pnpm --dir apps/tauri test
cargo test -p forge-tauri
```

The Phase 6 paint gate has no separate command: set
`localStorage.forgeTerminalDebug = "1"` in the WebView and the overlay's second
line reports `paint p50/p95/max` with the cell count, marking itself `OVER`
past the 8 ms budget.

### Phase 2 gate, last run

```
samples: 120 (lost 0)
key-to-delta: p50 9.8 ms · p95 10.5 ms · p99 16.6 ms · budget 50 ms
frame on the wire: mean 414 B · max 437 B
full repaint: 1148 B for 100x32 cells
```

## What is left

| Item                                    | Where                                                                                                                                          |
| --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| 10k-cell frame under 8 ms, **recorded** | Phase 6.6 — the overlay measures it and marks itself `OVER` past budget, but reading it needs a windowed run, so no number is written down yet |

Phase 6.1 is closed via fixture parity: `tests/fixtures/theme.json` is the
contract and `tokens.test.ts` asserts the TypeScript side equals it.

Everything else in the plan's §12 inventory is ✅ or explicitly deferred in
[PARITY.md](./PARITY.md).

## Notes

The frame size is the reason the rows are merged into style runs on the host
rather than shipped per cell: 3 200 cells as per-cell JSON is ~400 KB, and the
coalesce floor lets 62 of those through a second.
