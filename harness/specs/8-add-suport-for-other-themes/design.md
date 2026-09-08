# Design — feature 8 (add-suport-for-other-themes)

## Current architecture

Theme ownership stays in `apps/tauri/src/theme`/src/tokens.ts`. `Palette` holds
literal colors, derived interaction tokens are functions over that palette,
and `BASES` is the contract consumed by a future picker. `theme::apply` maps
the resolved palette into the UI kit. The UI reads theme tokens at render
time.

The Settings Personalization page is the likely integration point. The app
already persists a validated `ui.theme_base` preference and reapplies the
palette plus all-window refresh when that value changes. This means the likely
implementation is a small UI choice control backed by existing plumbing, plus
new palette data/tests/docs if genuinely new built-ins are requested.

## Proposed boundary pending approval

- New built-in palettes, if requested, belong in `theme tokens` and
  must be listed in `BASES` with a stable id, label, resolver branch, and
  contrast-conscious token values.
- Theme selection belongs in the Personalization page and should use the
  shared controls and theme tokens. It must not duplicate palette data in
  `ui`.
- Persistence should use the existing `ui.theme_base` app-state preference if
  the human approves cross-launch selection. A reset/follow-file choice would
  use the existing `None` semantics.
- `theme.json` precedence should remain unchanged unless explicitly approved:
  selected base first, then file color overrides, then palette application and
  window refresh.
- No protocol or SQLite migration is expected from the currently implemented
  preference path. This is a hypothesis to verify during implementation, not
  an authorization to add schema or wire types.

## Alternatives rejected for now

- Guessing a named theme list would turn an underspecified request into an
  arbitrary product decision.
- Adding a second color system in `ui` would violate the repository's single
  theme-owner invariant and make per-cell/per-frame rendering more expensive.
- Importing VS Code, TextMate, or another external theme format is not assumed:
  it would require a format contract, mapping rules for Forge-only semantic
  tokens, validation/error behavior, and likely a larger test and UI surface.

## Invariants

- `theme tokens` remains the only place that writes colors or metrics.
- Per-cell rendering continues to read the thread-local palette cache; no
  palette lock or deep copy is introduced on the render/delta paths.
- Theme switching remains a UI-side palette refresh and does not touch the
  daemon core lock, terminal engine, persistence schema, or network.
- Light themes, if approved, must also select appropriate light syntax assets
  and preserve readable semantic/status colors; this requires explicit design
  review rather than treating a light background as sufficient.
