# Theming

Forge ships nine built-in bases, plus a custom slot. Colors and control metrics live in
`apps/tauri/src/theme/tokens.ts`; views must not hardcode hex values. Interaction
states (hover, selection, borders, tints) are derived in `mix.ts` from `bg`,
`text` and the accents.

Default preference: **`forge-dark`**. A `system` preference follows
`prefers-color-scheme` (`forge-dark` / `forge-light`).

## Bases

| Id | Role |
|----|------|
| `forge-dark` | Default: warm dark `#1a1918` panels on a `#121211` window, `#ede5d6` text, verdigris `#4fa39b` accent |
| `forge-light` | The same roles on a `#faf8f3` ground, `#29766f` accent |
| `gruvbox-hard` | Gruvbox: `#282828` canvas, `#ebdbb2` text, `#458588` accent |
| `gruvbox` | Warm original Gruvbox medium-contrast surfaces |
| `neutral` | Neutral dark scale with Tokyo Night ANSI |
| `gruvbox-light` | Gruvbox light surfaces |
| `neutral-light` | Neutral light surfaces with violet focus |
| `ocean` | Deep blue surfaces with cyan accents |
| `forest` | Dark green surfaces with sage accents |

## Contract

`apps/tauri/tests/fixtures/theme.json` pins the literal palettes.
`tokens.test.ts` asserts the TypeScript side equals that fixture; regenerate
deliberately with:

```sh
bun run --cwd apps/tauri theme:fixture
```

Do not regenerate as part of the build — an auto-updated fixture asserts nothing.

## Runtime switch

`ThemeProvider` applies `data-theme` / `color-scheme` and CSS variables from
`toCssVariables` / `applyTheme`. The in-app selector (Settings → Personalization, and
the `?gallery` surface) changes the running preference; persistence goes through
app state like any other UI preference.

**Gruvbox** uses warm gray `#32302f` panels and `#3c3836` raised controls over
the `#282828` canvas, with cream text and a teal accent. Its saved id remains
`gruvbox-hard` so existing preferences continue to select it. Its Git colors follow the default
`groups` mapping in
[motaz-shokry/gruvbox.nvim](https://gitlab.com/motaz-shokry/gruvbox.nvim/-/blob/main/lua/gruvbox/config.lua)
(`gitConflict` uses `git_merge`). Forge derives its own interaction and syntax
styles. Filled controls use a foreground chosen for their fill, falling back to
black or white when necessary to reach 4.5:1, including hover and pressed states.

### Two hues, two jobs

The accent (`--accent`) carries every primary action, selection, focus ring,
active pane and link. Ember (`--attention`, the palette's `needsYou`) marks
exactly two things: a session waiting on the user and an unresolved conflict.

Selection never draws a bar down an edge. A chosen row changes its own ground
(`--select-bg`) and carries an inset hairline (`--select-ring`), so nothing
shifts when it is picked; an attention row takes the same shape in ember
(`--attention-bg`, `--attention-ring`) with a dot and a 3px halo. A selected
card warms its border (`--card-select-border`), gains an outer ring
(`--card-select-halo`) and fills its identifier chip. Tabs keep their underline,
because they sit on a shared edge.

### Semantic foregrounds

- `--*-solid` names a fill; `--*-fg` is the text **on that fill**.
- `--accent-text`, `--danger-text`, `--success-text`, `--warning-text`,
  `--attention-text` and `--info-text` are readable foregrounds on UI surfaces.
- `--git-*-text` serves Git labels and file decorations; raw `--forge-git-*`
  colours remain the inputs for diff washes and gutter marks.
- `--fg-default`, `--fg-muted` and `--fg-subtle` are derived from the palette,
  with a 4.5:1 floor across panels, overlays and interaction backgrounds.
- `--ring` is opaque and held to 3:1 against those same backgrounds.

These adjustments run when applying a theme. Raw palette values and terminal
ANSI colours stay available for export and terminal rendering. Tests cover every
base at every integer contrast setting from 0 to 100. Custom palettes
use the same derivation, but a palette mixing incompatible dark and light
surfaces cannot guarantee one foreground readable on all of them.

## Fonts

Three faces, all bundled under `apps/tauri/public/fonts/` (SIL OFL) so a
packaged app never reaches a CDN: IBM Plex Sans for the chrome (latin subset,
400/500/600 and italic), JetBrains Mono for code, the terminal, paths, counts
and keys, and Fraunces for documentation headings only (settings section
titles). JetBrains Mono ships as the full TTFs rather than a latin subset
because the terminal needs its box-drawing and block glyphs. Prompt icon glyphs (Nerd Font /
powerline codepoints such as U+E718) are **not** in that bundle — shipping a
Nerd Font patch would roughly 10× the font payload.

Install [JetBrainsMono Nerd Font](https://github.com/ryanoasis/nerd-fonts)
(the Mono variant) on the machine that runs the GUI. The mono stack prefers
`JetBrainsMono Nerd Font Mono` when present and falls back to the bundled
face for everything else.

## Controls and chrome

Density (`ui.density`: `compact` 26px rows or `cozy` 32px; the retired
`default`/`comfortable` ids map onto them) overlays row height and the control
ladder; UI font size (`ui.font_size`, half-pixel steps from 12 to 16) overlays
`--forge-text-*`. Reduce motion (`ui.reduce_motion`) sets
`data-reduce-motion` on the root, which removes every transition on top of the
OS setting.

The type scale is 28/1.2/600 section title (`--text-2xl`), 20/1.3/600 panel
heading, 15/1.45/600 card and dialog heading, 13/1.6 body, 12.5 row label, 11.5/1.5
caption and a 10px bold uppercase mono section label (`--text-label`); nothing is
set below 10px. Space steps are 4/8/12/16/22/32/44 (`--space-*`, pixel-named);
radii are 5 badge, 7 button and row, 9 card, 12 panel, 14 palette and a pill;
depth runs 0–3 (only menus, tooltips, dialogs and the palette cast a shadow);
motion is 0/90/120/160ms. Neither
moves content: the editor keeps `ui.editor.font_size` and every terminal and
agent shares `ui.terminal.zoom`. A theme switch reapplies both overlays from
`dataset` so they survive `applyTheme` rewriting the metric variables.

Control heights and radii come from the same token module (`CONTROL_XS`…`LG`).
Component look lives in `apps/tauri/src/ui/ui.css`, keyed off those CSS
variables. `styles/index.css` loads global defaults, then the control kit, then
feature layouts. Use control sizes and variants instead of redefining their
paint or focus states in a feature stylesheet.

Icons tint with `currentColor` against token-driven text colors —
with one deliberate exception, the language marks on file rows (`LangIcon`),
which are JetBrains' file-type icons and carry their own fills. They are
vendored in a dark and a light set under `apps/tauri/public/icons/lang`, chosen
by `baseIsLight`, because their colour is the information they carry and a
tint would flatten every file type to one hue. `langIconUrl` is the one place
that names the file, so the `LangIcon` component and the file tree — which
paints its rows imperatively, through the package's `icon` hook — can never
disagree about which artwork a name gets.

## How to extend

1. Add a palette to `palettes` in `tokens.ts`.
2. Update `ThemeBaseId`, `THEME_LABELS`, and any light-base sets.
3. Regenerate `tests/fixtures/theme.json`.
4. Cover derived tokens in `mix.test.ts` / `tokens.test.ts` if formulas change.

## Custom JSON themes

Settings → Personalization includes a live code diff and terminal preview, a
theme selector, editable accent/background/foreground colors, and a 0–100
contrast slider. Colors accept a native picker or a six-digit hex value.
Contrast defaults to 60, preserves those three colors, and adjusts panel
separation and secondary text. A drag previews locally and saves on release.
Choose a built-in theme again to restore its original palette.

**Import** accepts a JSON file; **Copy theme** copies the active theme as JSON.
The **Theme JSON** disclosure provides **Download theme** and a field for
pasting JSON followed by **Import and apply**. The document has `name`
(1–60 characters), `mode` (`light` or `dark`), and `palette` with all fields
from the downloaded theme.
Every color must use `#RRGGBB`; `ansi` must contain exactly sixteen colors.
Files are limited to 16 KiB and checked before reading. Imported colors are
validated structurally; readability depends on the colors you choose.

Edited variants also include `adjustments`, containing the unadjusted `palette`
and integer `contrast`. Both survive import, export, and restart so returning
the slider to 60 restores exact colors without accumulating rounding errors.

One custom palette is retained in `ui.custom_theme`, even when a built-in is
selected. Importing another replaces that slot. The active custom document is
also stored in `ui.theme_base`, so restoring the preference restores its colors
without depending on load order. Invalid saved preferences fall back to the
default. A selection applies immediately. If the command cannot be submitted, the
previous palette is restored and an error is shown. Daemon-side failures use
the runtime notice channel; acknowledged writes update the host snapshot.
