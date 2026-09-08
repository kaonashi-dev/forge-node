# Theming

Forge ships seven built-in bases. Colors and control metrics live in
`apps/tauri/src/theme/tokens.ts`; views must not hardcode hex values. Interaction
states (hover, selection, borders, tints) are derived in `mix.ts` from `bg`,
`text` and the accents.

Default preference: **`gruvbox-hard`**. A `system` preference follows
`prefers-color-scheme` (`gruvbox-hard` / `gruvbox-light`).

## Bases

| Id | Role |
|----|------|
| `gruvbox-hard` | Default dark: flat `#1f1f1f` canvas, Gruvbox semantic row |
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
pnpm --dir apps/tauri exec vite-node scripts/emit-theme-fixture.ts
```

Do not regenerate as part of the build — an auto-updated fixture asserts nothing.

## Runtime switch

`ThemeProvider` applies `data-theme` / `color-scheme` and CSS variables from
`toCssVariables` / `applyTheme`. The in-app picker (Settings → Appearance, and
the `?gallery` surface) changes the running preference; persistence goes through
app state like any other UI preference.

## Controls and chrome

Control heights and radii come from the same token module (`CONTROL_XS`…`LG`).
Component look lives in `apps/tauri/src/ui/ui.css`, keyed off those CSS
variables. Icons tint with `currentColor` against token-driven text colors —
with one deliberate exception, the language marks on file rows (`LangIcon`),
which are JetBrains' file-type icons and carry their own fills. They are
vendored in a dark and a light set under `apps/tauri/public/icons/lang`, chosen
by `baseIsLight`, because their colour is the information they carry and a
tint would flatten every file type to one hue.

## How to extend

1. Add a palette to `palettes` in `tokens.ts`.
2. Update `ThemeBaseId` and any light-base sets.
3. Regenerate `tests/fixtures/theme.json`.
4. Cover derived tokens in `mix.test.ts` / `tokens.test.ts` if formulas change.

## Custom JSON themes

Settings → Personalization → Custom theme accepts a JSON file or pasted JSON.
Use **Download template** to export the active palette, edit it, then choose
**Import and apply**. The document has `name` (1–60 characters), `mode`
(`light` or `dark`), and `palette` with all fields from the downloaded template.
Every color must use `#RRGGBB`; `ansi` must contain exactly sixteen colors.
Files are limited to 16 KiB and checked before reading. Imported colors are
validated structurally; readability depends on the colors you choose.

One custom palette is retained in `ui.custom_theme`, even when a built-in is
selected. Importing another replaces that slot. The active custom document is
also stored in `ui.theme_base`, so restoring the preference restores its colors
without depending on load order. Invalid saved preferences fall back to the
default. A selection applies immediately. If the command cannot be submitted, the
previous palette is restored and an error is shown. Daemon-side failures use
the runtime notice channel; acknowledged writes update the host snapshot.
