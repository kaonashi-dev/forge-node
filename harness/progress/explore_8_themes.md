# Exploration — feature 8 (`add-suport-for-other-themes`)

## Request

The user supplied the exact feature text `Add suport for other themes`.

## Repository findings

- `apps/tauri/src/theme/tokens.ts` is the sole owner of color and metric
  tokens. It defines the `Palette` ramp, derived interaction colors, terminal
  ANSI colors, runtime `configure_with_base`, and the advertised `BASES` list.
- Two built-in bases currently exist: `gruvbox` / `Gruvbox Dark` (default) and
  `neutral` / `Neutral Dark`. The file-only alias `neutral-dark` is accepted
  when reading `theme.json` but is not advertised as a picker id.
- `theme.json` is optional, lives beside `config.toml`, and can select a base
  plus override individual colors or leading ANSI entries. Unknown JSON keys
  invalidate the file and fall back safely; Forge never writes the file.
- `apps/tauri/src/theme/src/lib.rs` resolves the theme before applying it to
  the GUI-component, so borrowed controls and Forge-owned views share the same
  palette.
- `apps/tauri exposes a Personalization page that reports the
  active base but currently renders no selection control. It already reads a
  validated `ui.theme_base` preference from the daemon snapshot.
- `apps/tauri already notices changes to `ui.theme_base`, calls
  `theme::configure_with_base`, reapplies the the GUI-component theme, and
  refreshes all windows. The preference write path uses the existing opaque
  app-state request; no new protocol is apparent from the current design.
- Existing unit tests cover base resolution, overrides, aliases, invalid ids,
  palette replacement, and the contract that every `BASES` entry is distinct
  and accepted by the resolver.
- `docs/theming.md` and the Personalization section of `docs/ui.md` describe
  the current behavior and explicitly say selection controls can be added
  when the product supports them.

## Scope conclusion

The request identifies a desired direction but not the theme catalog or the
expected behavior. “Other themes” could mean adding named built-in palettes,
exposing the existing Neutral base in Settings, importing external theme
formats, or improving the current `theme.json` workflow. These choices change
the UI, data contract, documentation, and test plan. The spec therefore
records the likely Settings integration as a candidate and stops at a human
gate rather than inventing a theme list or an external format.

No production files should be changed until the human confirms the intended
theme names/source and whether runtime-only selection, persisted selection,
custom `theme.json` overrides, or import/export are required.
