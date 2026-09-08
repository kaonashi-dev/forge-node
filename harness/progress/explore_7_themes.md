# Investigation — feature 7 (add-support-for-toher-themes)

## Request

The exact `spec_raw` is `Add support for toher themes`.

The wording appears to contain a typo (`toher`) and does not identify the
themes or the desired input/selection model. The spec therefore records the
ambiguity for human approval rather than choosing a catalog silently.

## Existing implementation

- `apps/tauri/src/theme/tokens.ts` defines the complete `Palette` with Forge
  surfaces, text, semantic colors, Git decoration colors, terminal colors, and
  16 ANSI colors.
- Built-in `BASES` currently contains `gruvbox` / “Gruvbox Dark” and `neutral`
  / “Neutral Dark”. `theme.json` can choose a base and override individual
  colors or up to 16 ANSI entries.
- `configure_with_base()` resolves the base, publishes the palette and active
  id, and bumps the generation used by the per-thread render cache.
- `theme::apply()` pushes the resolved colors into `the GUI-component`, including
  editor syntax colors and control colors.
- `apps/tauri parses and persists `ui.theme_base`, filters
  unknown ids, and has a Personalization page showing the current label. It
  does not currently expose a theme choice on that page.
- `apps/tauri detects a changed persisted preference, calls
  `configure_with_base`, reapplies the the GUI theme, and refreshes windows.

## Boundaries

Themes are local presentation state. The daemon, protocol, domain model,
SQLite schema, terminal engine, and client wire replica should not be involved.
Literal colors and palette resolution stay in `theme tokens`; UI
interactions use the controls boundary. The theme read path is already
optimized for the per-cell render rung and must not be replaced with a lock or
allocation per cell.

## Open decision for the gate

Approve or revise the recommended candidate catalog (Solarized Dark, Tokyo
Night, Catppuccin Mocha), and confirm whether the feature includes only
bundled selectable bases or also an imported theme format. The requirements
remain intentionally conditional on that decision.

