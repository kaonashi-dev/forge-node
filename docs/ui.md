# UI

Forge's window is the Tauri + Solid shell in `apps/tauri`. The host
(`apps/tauri/src-tauri`) owns the runtime thread, workbench worker, and cell
encoder; the WebView renders chrome and a Canvas terminal from a passive
`client::CellGrid`. There is no second VT engine (ADR-011).

Day-to-day layout and behaviour details live next to the code:

| Area | Where |
|------|--------|
| Shell (title, rail, tabs, status) | `apps/tauri/src/shell/` |
| Panels (files, git, history, PR, harness) | `apps/tauri/src/panels/` |
| Workbench (editor, diff, feature, PR) | `apps/tauri/src/workbench/` |
| Terminal Canvas | `apps/tauri/src/terminal/` |
| Control kit | `apps/tauri/src/ui/` |
| Theme tokens | `apps/tauri/src/theme/` + [theming.md](./theming.md) |
| Actions / keymap | `apps/tauri/src/actions/` |
| Feature checklist | `apps/tauri/PARITY.md`, `apps/tauri/PROGRESS.md` |
| App README | `apps/tauri/README.md` |

## Layout (summary)

```text
┌──────────────────────────────────────────────────────────────┐
│ title bar + session tabs                                      │
├─────────────────────┬────────────────────────────────────────┤
│ Projects rail       │  centre: terminal / workbench tabs      │
│                     │                                         │
├─────────────────────┴────────────────────────────────────────┤
│ status bar                                                    │
└──────────────────────────────────────────────────────────────┘
```

Right inspector tabs (files, git, history, PRs, features, lieutenant) share one
resizable column. Theme metrics (`TITLE_H`, rail/panel defaults, control ladder)
come from `theme/tokens.ts`, never hardcoded hex in views.

## Terminal

Damaged rows arrive on `runtime:cells` separately from `runtime:state`, so a
frame of output never serializes the session tree with it. The Canvas renderer
paints style runs; selection, clipboard, IME, and mouse reporting live under
`src/terminal/`. Latency gate: `make latency-tauri` (p95 ≤ 50 ms).

## Controls

Call sites import from `apps/tauri/src/ui/` (Button, Dialog, Menu, …). Look is
Forge CSS + tokens; interaction primitives are `@kobalte/core` behind that
boundary. Structural chrome (tab strip, tree row, palette row) stays in shell /
palette modules on purpose.

## Keyboard

Chords resolve through `actions/` → dispatch. Settings → Keyboard and the
command palette surface the same bindings. Terminal input mapping for the PTY
stays in `terminal-input` / the host bridge, not in Solid.
