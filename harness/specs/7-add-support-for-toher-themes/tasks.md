# Tasks — feature 7 (add-support-for-toher-themes)

- [ ] **T1 — Resolve the product scope at the human gate**
      Approve the exact additional theme names/ids, whether they are bundled,
      and whether `theme.json` remains an override layer for each base.
- [ ] **T2 — Extend the theme registry**
      Add the approved complete palettes to `theme tokens`, preserve
      safe fallback behavior, and test ids, labels, distinctness, and all
      required color roles.
- [ ] **T3 — Add the Personalization selector**
      Expose the approved themes using the existing controls boundary and write
      the selected id through `ui.theme_base`; retain an explicit current
      state and keyboard/accessibility behavior consistent with existing UI.
- [ ] **T4 — Verify runtime and configuration behavior**
      Confirm startup restoration, live re-application to the theme,
      derived tokens, ANSI/terminal colors, JSON overrides, and invalid-input
      fallback without adding render-rung work.
- [ ] **T5 — Tests and documentation**
      Add focused unit/UI coverage and document the approved ids and
      `theme.json` contract where users discover configuration.
- [ ] **T6 — Full gate**
      Run `./init.sh` without `--fast` and record the result in
      `harness/progress/impl_7.md`.

