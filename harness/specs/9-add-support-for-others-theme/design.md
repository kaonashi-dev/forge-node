# Design — feature 9 (add-support-for-others-theme)

## Current architecture

Theme ownership is already centralized in `apps/tauri/src/theme`/src/tokens.ts`.
`Palette` is a `Copy` value containing all literal color roles. `BASES` maps
stable ids to labels, `base_palette` maps ids to built-in values, and
`configure_with_base` layers `theme.json` overrides over the chosen base before
publishing it through the generation-tagged palette cache. `apps/tauri` reads
that registry in Settings and applies changes with `theme::apply` plus a refresh
of all windows. The persisted base selection is an existing app-state value,
`ui.theme_base`.

## Proposed design, pending clarification

The implementation should extend this existing registry and resolver rather than
introducing a parallel theme abstraction. For built-in themes, the likely shape
is:

- add one complete `Palette` constant per approved theme;
- add its stable id and display label to `BASES`;
- add the id-to-palette mapping in `base_palette`;
- preserve `theme.json` as an optional override layer;
- expose the approved choices through the existing Personalization interaction;
- test resolution, fallback, persistence, override precedence, and live
  repaint behavior at the appropriate crate boundaries.

This is intentionally not a final design because the request does not identify
the themes or the desired selection surface. Light/system themes could require
additional decisions about contrast, platform appearance observation, and
default selection; arbitrary imports would require a different validation and
storage UX than adding fixed constants.

## Rejected approaches

- Do not add colors in individual views. That violates the repository's single
  theme-source invariant and would leave terminal, syntax, diff, and semantic
  colors inconsistent.
- Do not make the GUI own a separate theme object or add a second terminal
  palette. The daemon remains the terminal owner, and the existing palette
  functions are deliberately hot-path optimized.
- Do not choose a popular theme (for example, Solarized or Tokyo Night) on the
  user's behalf. The request gives no such product decision, and choosing one
  would make the spec appear approved while silently changing the desired
  scope.

## Invariants

- Read `docs/performance.md` before implementation because palette reads are
  used in per-cell rendering.
- Keep `theme tokens` as the only color/metric source.
- Preserve `Palette: Copy`, generation invalidation, and thread-local caching.
- Preserve safe fallback for unknown ids and malformed `theme.json` values.
- Do not add dependencies, migrations, protocol changes, daemon work, or
  filesystem writes unless the human approves an expanded requirement.
