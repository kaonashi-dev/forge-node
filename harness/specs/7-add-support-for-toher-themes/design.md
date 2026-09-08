# Design — feature 7 (add-support-for-toher-themes)

## Current state

`apps/tauri/src/theme/tokens.ts` is the palette boundary. It owns the `Palette`
data, the built-in bases registry, derived interaction tokens, and CSS variable
application. Settings Personalization / ThemeProvider already read those bases
and persist `ui.theme_base`.

## Proposed implementation shape

- Extend the single bases registry and its palette resolver for the names
  approved at the gate; do not create a second theme abstraction.
- Keep all literal colors in `tokens.ts`. Continue deriving hover, selection,
  focus, border, syntax, terminal and semantic variants from that palette.
- Add the selection interaction in Settings → Appearance.
- Keep fixture parity via `tests/fixtures/theme.json` and `tokens.test.ts`.
