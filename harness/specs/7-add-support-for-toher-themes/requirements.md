# Requirements — feature 7 (add-support-for-toher-themes)

## Verbatim request

`Add support for toher themes`

## Scope decision required at the human gate

The request does not name the themes to add or define whether “support” means
bundled palettes, a user-facing selector, imported theme files, or all three.
The current implementation already supports two bundled dark bases (`gruvbox`
and `neutral`) plus `theme.json` color overrides, but the Personalization page
only displays the current base. The implementation must not invent a theme
catalog without approval. The recommended concrete scope is:

- add an approved list of named bundled themes (candidate examples: Solarized
  Dark, Tokyo Night, and Catppuccin Mocha); and
- expose that list in Personalization, persist the selected base through the
  existing `ui.theme_base` preference, and apply it consistently at runtime.

The human gate should approve or revise the candidate names and whether
`theme.json` remains an override layer for every bundled base.

## R1 — Approved theme catalog

The system SHALL define a stable id and display label for each additional
theme approved at the human gate, alongside the existing `gruvbox` and
`neutral` entries in `theme tokens::BASES`. Each id SHALL resolve to a
distinct complete palette covering Forge chrome, semantic colors, terminal
colors, Git decorations, and the ANSI ramp.

## R2 — User selection and persistence

WHEN a user selects an approved theme in Personalization, the system SHALL
persist its id using the existing application preference mechanism and SHALL
restore that selection on the next launch. The default and an absent or
unknown stored id SHALL preserve the current safe fallback behavior.

## R3 — Consistent runtime application

WHEN the active theme changes, the system SHALL re-resolve the process-global
palette, re-apply the `the UI kit` theme, refresh all affected windows,
and derive interaction and syntax colors from the selected palette. No view
may retain a stale hard-coded color or a previous theme's derived token.

## R4 — Configuration compatibility and failure safety

If `theme.json` supplies valid color overrides, the system SHALL layer them
over the selected base according to the approved configuration contract. If
the file is absent, malformed, contains an unknown base, or contains invalid
color values, Forge SHALL remain launchable and SHALL fall back to the safest
valid base/field values while reporting the problem through existing logging.

## R5 — Verification

The system SHALL include focused tests for catalog identity, palette
completeness/distinctness, persistence and selection, runtime re-application,
override layering, and invalid-input fallback. The full repository gate SHALL
pass before the feature is closed.

## Traceability

| Acceptance | Requirement |
| --- | --- |
| Approved additional themes are exposed beside the existing bases | R1, R2 |
| Selection persists and restores after restart | R2 |
| Chrome, controls, editor, terminal, ANSI, and derived states change together | R1, R3 |
| Invalid identifiers/files do not prevent startup | R4 |
| Focused tests and the canonical gate pass | R5 |

