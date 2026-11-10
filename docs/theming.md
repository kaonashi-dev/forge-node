# Theming

Forge ships eleven built-in bases plus one custom slot. The source of truth is
`apps/ui/src/theme/themes.json`; `apps/ui/src/theme/roles.rs` derives every
semantic role from it, and `theme/mod.rs` seeds a `rune_kit::Kit`. A view never
writes a hex value: it reads a `Kit` colour or metric.

Default preference: **`calm-dark`**. A `system` preference follows the OS
appearance (`calm-dark` / `calm-light`). A stored preference is kept, so an
existing install stays on the base it chose.

## Bases

| Id | Role |
|----|------|
| `calm-dark` | Default: `#131315` card on a `#0c0c0d` window, `#ededef` text over `#c9c9cf` rows, violet `#a78bfa` accent, ember `#f59e4b` |
| `calm-light` | The same roles on a `#f4f4f2` window and `#ffffff` card, `#6d28d9` accent, `#a04a08` ember |
| `forge-dark` | Warm dark `#1a1918` panels on a `#121211` window, `#ede5d6` text, verdigris `#4fa39b` accent |
| `forge-light` | The same roles on a `#faf8f3` ground, `#29766f` accent |
| `gruvbox-hard` | Gruvbox: `#282828` canvas, `#ebdbb2` text, `#458588` accent |
| `gruvbox` | Warm original Gruvbox medium-contrast surfaces |
| `neutral` | Neutral dark scale with Tokyo Night ANSI |
| `gruvbox-light` | Gruvbox light surfaces |
| `neutral-light` | Neutral light surfaces with violet focus |
| `ocean` | Deep blue surfaces with cyan accents |
| `forest` | Dark green surfaces with sage accents |

## Contract

`themes.json` pins the literal palettes. `roles::derive` is the one place that
turns a palette into panel, border, muted and git colours; `roles.rs` tests
cover every base. To change a base, edit `themes.json`, run the Rune gate, and
update the preview if its layout changed — the derivation has no generated
intermediate and no drift test.

## Runtime switch

`AppState` holds `ui.theme_base` (a base id, `"system"` or a custom slot) and
the custom document in `ui.custom_theme`. `ui.rs` seeds the `Kit` from the
resolved base each frame; `appearance.rs` owns the picker, the live
`ThemePreview`, the accent, contrast, import/export and the custom JSON
validation. A switch applies immediately; a failed `SetAppState` restores the
previous palette and shows the error.

**Gruvbox** uses warm gray `#32302f` panels and `#3c3836` raised controls over
the `#282828` canvas, with cream text and a teal accent. Its saved id remains
`gruvbox-hard` so existing preferences continue to select it. Its Git colours
follow the default `groups` mapping in
[motaz-shokry/gruvbox.nvim](https://gitlab.com/motaz-shokry/gruvbox.nvim/-/blob/main/lua/gruvbox/config.lua)
(`gitConflict` uses `git_merge`). Interaction and syntax styles are Forge's.
Filled controls choose a foreground for their fill, falling back to black or
white to reach 4.5:1, including hover and pressed states.

### Two hues, two jobs

The accent carries every primary action, selection, focus ring, active pane and
link. Ember (`needs_you`) marks exactly two things: a session waiting on the
user and an unresolved conflict.

Selection never draws a bar down an edge. A chosen row changes its own ground
(the accent at 14%) and nothing else, so nothing shifts when it is picked; an
attention row takes the same shape in ember with a dot and a 3px halo. A
selected card warms its border and gains an outer ring. A tab is chosen by a
raised chip; the open-files strip keeps an accent underline. The card's tab
strip uses the base's `strip` ground and `strip_rule`, derived in `roles.rs`.

### Semantic foregrounds

- `*-solid` names a fill; `*-fg` is the text **on that fill**.
- `accent_text`, `danger_text`, `success_text`, `warning_text`,
  `attention_text` and `info_text` are readable foregrounds on UI surfaces.
- `git_*_text` serves Git labels and file decorations; raw git colours remain
  the inputs for diff washes and gutter marks.
- `text_base` (primary), `text_muted` (rows) and `text_faint` (icons) are
  derived from the palette with a 4.5:1 floor across panels, overlays and
  interaction backgrounds. A base may pin overrides in `themes.json`.
- `focus` is opaque and held to 3:1 against those same backgrounds.

These adjustments run when a theme is resolved. Raw palette values and terminal
ANSI colours stay available for export and terminal rendering. Tests cover every
base at every integer contrast setting from 0 to 100.

## Fonts

Geist for the chrome (400/500/600 and italic), Geist Mono for code, the
terminal, paths, counts and keys, and JetBrains Mono as the per-glyph fallback.
Geist and Geist Mono ship under `apps/ui/assets/` (SIL OFL); JetBrains Mono is
the system fallback. Braille, `❯`, `⎿` and similar fall through to JetBrains
Mono. Prompt icon glyphs (Nerd Font / powerline codepoints such as U+E718) are
**not** bundled — shipping a Nerd Font patch would roughly 10× the font payload.

Install [JetBrainsMono Nerd Font](https://github.com/ryanoasis/nerd-fonts) (the
Mono variant) on the machine that runs the GUI. The mono stack tries it after
Geist Mono, so its icons fill the codepoints Geist Mono lacks.

## Controls and chrome

Density (`ui.density`: `compact` or `cozy`) changes row heights and the control
ladder. Trim (`ui.trim`) is a finish beside Glass, not a density: it lowers
chrome heights, tightens layout gaps and flattens radius rungs while leaving
row height, grid padding and circles alone. Glass (`ui.glass`, dark only) uses
translucent chrome tokens and Rune's system material. UI font size
(`ui.font_size`, half-pixel steps from 12 to 16) scales the chrome type;
`ui.reduce_motion` removes transitions and spinner motion.

The type scale is Rune's `Kit.ty`: page title, headings, 13px rows/tabs/body,
12px captions, and mono for branches, paths, counts and keys. Space and radii
come from the kit; the shell's strip, card and footer heights are derived in
`theme/roles.rs` from `ChromeMetrics`. The editor keeps
`ui.editor.font_size` and line spacing; every terminal and agent shares
`ui.terminal.zoom` (50–200 %).

## Icons

Every mark is Forge's own 59-glyph set on one 16-grid (`apps/ui/src/glyphs.rs`).
Language marks are the 87 file-type icons in `file_icons.rs`, vendored dark and
light under `apps/ui/assets/lang/` (JetBrains artwork, Apache-2.0; see the
LICENSE there). They keep their own colours because the colour is the
information; a file row picks the set by the base's lightness. Brand marks are
the provider exception.

## How to extend

1. Add the palette to `apps/ui/src/theme/themes.json`.
2. Add its id to the base enum/labels in `theme/mod.rs`.
3. Add a `roles.rs` derivation case if it is not a plain colour read.
4. Run `scripts/dev ui check`; the role tests cover every base.

## Custom JSON themes

Settings → Appearance includes a live code diff and terminal preview, a theme
selector, editable accent/background/foreground colours, and a 0–100 contrast
slider. Colours accept a text field (six-digit hex) or the custom editor.
Contrast defaults to 60, preserves those three colours, and adjusts panel
separation and secondary text. A drag previews locally and saves on release.
Choose a built-in theme again to restore its original palette.

**Import** accepts a JSON file; **Copy theme** copies the active theme as JSON.
The document has `name` (1–60 characters), `mode` (`light` or `dark`), and
`palette` with every field from the downloaded theme. Every colour must use
`#RRGGBB`; `ansi` must contain exactly sixteen colours. Files are limited to
16 KiB and checked before reading. Imported colours are validated structurally;
readability depends on the colours you choose.

Edited variants include `adjustments`, containing the unadjusted `palette` and
integer `contrast`, so returning the slider to 60 restores exact colours
without accumulating rounding errors.

One custom palette is retained in `ui.custom_theme`, even when a built-in is
selected. Importing another replaces that slot. The active custom document is
also stored in `ui.theme_base`, so restoring the preference restores its colours
without depending on load order. Invalid saved preferences fall back to the
default.
