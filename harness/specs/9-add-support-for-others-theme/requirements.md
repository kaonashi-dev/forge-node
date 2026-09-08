# Requirements — feature 9 (add-support-for-others-theme)

The specification input is preserved verbatim:

> Add support for others theme

The request is underspecified. The requirements below establish the safe
boundary for the feature preparation and identify the decisions required before
production implementation.

## R1 — Verbatim request and scope

The feature record and this specification MUST preserve the exact request
`Add support for others theme`. No particular palette, theme name, theme source,
or light/dark behavior may be inferred from that sentence alone.

## R2 — Named theme selection

Before implementation begins, the approved scope MUST identify every additional
theme by a stable id and user-facing label, and MUST state whether the themes
are built-in, imported from user files, or both.

## R3 — Palette contract

If the approved scope adds built-in themes, each theme MUST resolve through the
existing `theme tokens` palette contract, cover the fields needed by
Forge's chrome, terminal, semantic state, Git decoration, syntax, and ANSI
rendering, and remain compatible with the existing `theme.json` override layer.

## R4 — Selection and live application

If the approved scope exposes theme selection in the UI, selecting a valid
theme MUST update the existing persisted preference and apply the resolved
palette to every open window without restarting the daemon or creating a second
theme owner. Invalid or unavailable selections MUST retain the current safe
fallback behavior.

## R5 — Performance and boundaries

The implementation MUST preserve the palette generation/thread-local cache and
the rule that per-cell rendering allocates nothing and takes no lock. It MUST
not add protocol messages, daemon behavior, a second terminal engine, or a
database migration unless a later approved clarification explicitly expands the
scope.

## R6 — Preparation-only gate

This `/feature` preparation MUST change only harness artifacts. It MUST NOT
modify production Rust code, dependencies, protocol definitions, persistence
schema, or runtime behavior.

## Traceability

| Acceptance | Requirement |
|---|---|
| The exact request is preserved and no palette is guessed | R1 |
| The gate asks for named themes, ids, labels, and source model | R2 |
| Any built-in palette uses the existing complete palette and override contract | R3 |
| A UI selection, if approved, persists and repaints all windows safely | R4 |
| Hot-path, daemon, protocol, and persistence boundaries remain explicit | R5 |
| No implementation is performed before approval | R6 |
