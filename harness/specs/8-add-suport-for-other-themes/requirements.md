# Requirements — feature 8 (add-suport-for-other-themes)

## Verbatim request

`Add suport for other themes`

The request is intentionally preserved with its original spelling. The
behavioral target is not yet specific enough to approve implementation: the
repository already has two built-in bases and a JSON override mechanism.

## R1 — Theme catalog (requires clarification)

The product MUST identify the additional theme(s) to support, including their
stable ids and user-facing labels, before implementation begins. A theme may
be a new built-in palette or an explicitly named external/imported format;
these are different scopes.

## R2 — Selection behavior (requires clarification)

When the approved theme catalog contains selectable built-in themes, the
Personalization surface MUST offer each supported theme and visibly identify
the active one. The approval must specify whether selection applies only to the
running process or persists as `ui.theme_base`.

## R3 — Palette consistency

Any approved theme MUST resolve through `theme tokens`, cover the
palette tokens required by the shell and terminal, and keep derived
interaction states and the UI kit colors consistent with the selected
palette. Views MUST NOT gain hardcoded color values.

## R4 — Configuration precedence (requires clarification)

The approval must specify how a selected built-in theme interacts with
`theme.json`'s `base` and color overrides. Existing behavior gives an explicit
picker choice precedence over the file's base while retaining file overrides;
changing that behavior requires an explicit decision.

## R5 — Verification and documentation

The implementation MUST add or update deterministic tests for theme resolution
and selection behavior, and update `docs/theming.md` / `docs/ui.md` so the
supported catalog and configuration behavior are discoverable.

## Acceptance traceability

| Acceptance | Requirement |
|---|---|
| The supported additional themes have stable ids and labels | R1 |
| The approved selection surface shows and applies the supported themes | R2 |
| All shell, terminal, interaction, and borrowed component colors follow the selected palette | R3 |
| `theme.json` and persisted preference precedence is documented and tested | R4 |
| Tests and user-facing theming documentation cover the final behavior | R5 |

## Open decisions for the human gate

1. Which themes should be added or exposed? (For example, expose the existing
   Neutral base only, add named built-ins such as a light theme, or support an
   external theme format.)
2. Should the theme choice persist across launches, remain runtime-only, or do
   both with a reset/follow-`theme.json` option?
3. Should custom `theme.json` overrides remain supported and layered over every
   selected base?
