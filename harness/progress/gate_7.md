# Human gate — spec approval

**Feature:** 7 (add-support-for-toher-themes)
**Title:** Add support for toher themes

**Question:** Do you approve this spec for implementation?

**Context:**
- Requirements: 5 (R1 approved catalog, R2 selection/persistence, R3 runtime application, R4 configuration safety, R5 verification)
- Crates: ui, theme tokens, client
- Existing support: Gruvbox Dark and Neutral Dark bases, `theme.json` overrides, persisted `ui.theme_base`, and live palette re-application already exist.
- Design note: extend the single `theme tokens` registry and add the missing Personalization selector; do not involve the daemon or protocol.

**Caveat:** The original request is verbatim `Add support for toher themes`. It does not name the themes or specify bundled palettes versus imported theme files. The recommended scope proposes Solarized Dark, Tokyo Night, and Catppuccin Mocha as bundled selectable themes while preserving `theme.json` as an override layer. Approving means accepting those names and that scope; choose `revise` if a different catalog or import model is intended.

**Options:** `approve` | `revise` | `block`

**Spec:** `harness/specs/7-add-support-for-toher-themes/`

---
*Resolve with `/feature-go 7` (approve) or ask for changes before implementing.*

