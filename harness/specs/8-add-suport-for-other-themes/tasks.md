# Tasks — feature 8 (add-suport-for-other-themes)

These tasks are intentionally not executable until the human resolves the
open decisions in `requirements.md`.

- [ ] Confirm the theme catalog: names, stable ids, labels, dark/light
  appearance, and whether themes are built-in or imported.
- [ ] Confirm persistence semantics and the reset/follow-`theme.json` behavior.
- [ ] Confirm `theme.json` override precedence and whether its format remains
  the only custom-theme mechanism.
- [ ] Implement approved palette data and `BASES` resolution in
  `apps/tauri/src/theme`/src/tokens.ts`, with token/contrast and resolver tests.
- [ ] Add the approved selection interaction to the Personalization page using
  `theme tokens::controls`; connect it to the existing preference/apply path.
- [ ] Verify terminal ANSI colors, syntax light/dark asset selection where
  applicable, the UI kit theme mapping, and all-window repaint behavior.
- [ ] Update `docs/theming.md` and the relevant `docs/ui.md` section.
- [ ] Run focused theme tokens/UI tests, then `scripts/dev check`; record
  requirement-to-evidence mappings in `harness/progress/impl_8.md`.
