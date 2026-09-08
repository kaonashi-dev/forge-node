# Human gate — feature 8 (`add-suport-for-other-themes`)

Status: **NEEDS CLARIFICATION**

## Source request

`Add suport for other themes`

## Review before approval

Forge already supports two built-in bases (`Gruvbox Dark` and `Neutral Dark`),
runtime base resolution, persisted `ui.theme_base` plumbing, and arbitrary
color overrides through `theme.json`. The Personalization page currently only
reports the active theme. The request does not say whether the desired change
is to expose Neutral, add specific new built-in palettes, or import themes from
another format.

## Required human decision

Please specify:

1. The themes to support or expose, including names/ids if known.
2. Whether the choice persists across launches or is runtime-only.
3. Whether `theme.json` overrides remain layered over the selected base, and
   whether external theme import/export is in scope.

Approval is valid only after these decisions are incorporated into
`requirements.md`, `design.md`, and `tasks.md`; do not run `/feature-go 8` yet.

## Scope guard

This preparation changes only harness state and specification artefacts. No
production code, dependency, schema, protocol, or runtime behavior has been
implemented.

## Options

`approve` | `revise` | `block`

**Spec:** `harness/specs/8-add-suport-for-other-themes/`

---

*Resolve with `/feature-go 8` only after the clarification is incorporated.*
