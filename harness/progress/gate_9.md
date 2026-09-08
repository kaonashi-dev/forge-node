# Human gate — spec approval

**Feature:** 9 (add-support-for-others-theme)
**Title:** Add support for others theme

**Question:** Do you approve this spec for implementation, and can you clarify
the requested themes and selection model?

**Context:**

- Requirements: 6 (R1 verbatim scope, R2 named selections, R3 palette contract,
  R4 live selection, R5 performance/boundaries, R6 preparation-only)
- Theme: `apps/tauri/src/theme`
- Existing support: Forge already has Gruvbox Dark and Neutral Dark built-ins,
  `theme.json` overrides, persisted `ui.theme_base`, and live palette
  application. The Personalization page currently shows the active theme but
  does not provide a picker.
- Research: `harness/progress/explore_9_theme.md`
- Design: the likely extension is the existing `Palette`/`BASES` registry and
  Settings path, subject to the decisions below.

**Required clarification before implementation:**

1. Which themes should be added? Please provide names, stable ids, and labels.
2. Should they be built-in presets, user-imported theme files, or both?
3. Should light themes or system appearance following be included?
4. Should the Personalization page provide the selection control, or is editing
   `theme.json` sufficient?

**Safety boundary:** This preparation changes only harness artifacts. No
production code, dependency, protocol, persistence schema, or runtime behavior
has been changed, and implementation must not start until the above scope is
resolved.

**Options:** `approve` | `revise` | `block`

**Spec:** `harness/specs/9-add-support-for-others-theme/`

---
*Resolve with `/feature-go 9` (approve) or ask for changes before implementing.*
