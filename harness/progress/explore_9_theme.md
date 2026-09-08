# Exploration — feature 9 (add-support-for-others-theme)

## Request

The feature request is preserved verbatim as:

> Add support for others theme

The phrase does not name a theme, a theme source, or whether “others” means
additional built-in palettes or user-imported themes.

## Current implementation

- `apps/tauri/src/theme/tokens.ts` owns the complete palette contract. `Palette`
  contains the chrome, semantic, terminal, Git-decoration, and ANSI colors.
- `GRUVBOX_DARK` is the default built-in palette and `NEUTRAL_DARK` is the
  existing second built-in palette.
- `BASES` is the picker-facing registry: `(id, label)` pairs currently contain
  `gruvbox` / `Gruvbox Dark` and `neutral` / `Neutral Dark`.
- `configure_with_base` selects a registered base, overlays optional
  `theme.json` color values, updates the active-base id, increments the palette
  generation, and supports live replacement. Unknown ids fall back safely.
- `theme.json` is read from the platform config directory. It can choose a base
  and override named colors or up to 16 ANSI entries. It is never written by
  Forge.
- Derived interaction, syntax, identity, diff, and terminal tokens are exposed
  as functions over the active palette. This keeps views from owning literal
  colors and preserves the per-cell/thread-local palette-read cost model.
- `configure_with_base` must be followed by `theme::apply` and a refresh of all
  windows. `apps/tauri already does this when the persisted
  theme preference changes.
- `apps/tauri has a Personalization section and persists the
  selected base under `ui.theme_base`. `Prefs::from_store` ignores unknown ids.
  The current Personalization page only renders a readout; it does not expose a
  theme picker.
- The preference is metadata in the existing app-state store. No daemon,
  protocol, SQLite migration, terminal engine, or second palette owner is
  indicated by the current design.

## Relevant constraints

- `theme tokens` is the only place where colors and theme metrics are
  defined; views must use theme tokens and controls rather than hardcoded
  values.
- The palette is read on the render path, including terminal cells. A new
  theme must keep `Palette` copyable and preserve the generation/thread-local
  cache; no per-cell allocation or lock may be introduced.
- Built-in theme identifiers and labels must remain a single vocabulary shared
  by the resolver and the settings UI.
- User overrides must continue to layer over the selected base, and malformed
  overrides must not prevent the app from launching.
- The repository guide requires reading `docs/performance.md` before changing
  the render path or theme-related hot code. This feature preparation does not
  modify that path.

## Open product decisions

Before implementation, the human must specify:

1. Which named themes should be added, including their user-facing labels and
   stable ids.
2. Whether the request means additional built-in palettes, arbitrary imported
   theme files, or both.
3. Whether the new themes need only the existing dark-mode palette contract,
   or whether light/system themes and accessibility contrast requirements are
   also in scope.
4. Whether each new base needs a complete independent ANSI/semantic ramp or
   may inherit selected values from an existing base.
5. Whether the Personalization page should gain a selectable control now, or
   whether `theme.json` is the only selection surface.

## Likely implementation surface after clarification

Subject to the gate decisions, the smallest likely change is confined to
`theme tokens` (new `Palette` constants, `BASES`, resolver tests, and possibly
theme-file documentation) and `ui` (a picker/readout update and preference
handling tests). No new dependency is currently justified.
