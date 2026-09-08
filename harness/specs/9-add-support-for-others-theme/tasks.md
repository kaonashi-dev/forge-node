# Tasks — feature 9 (add-support-for-others-theme)

## Human-gate prerequisite

- [ ] Name every requested additional theme and approve its stable id and
      display label.
- [ ] Decide whether the feature is built-in presets, user-imported themes, or
      both.
- [ ] Decide whether light/system appearance and accessibility contrast are in
      scope.
- [ ] Decide whether theme selection belongs in the Personalization page,
      `theme.json`, or both.

## Implementation tasks after approval

- [ ] Read `docs/performance.md` and confirm the palette hot-path constraints.
- [ ] Add the approved palette data and registry entries in
      `apps/tauri/src/theme`/src/tokens.ts`.
- [ ] Preserve and test override precedence, unknown-id fallback, and complete
      ANSI/semantic coverage.
- [ ] Expose the approved choices through existing Settings controls and persist
      the selected id using the existing app-state preference if UI selection is
      in scope.
- [ ] Verify live application refreshes all windows and does not add daemon,
      protocol, migration, or per-cell overhead.
- [ ] Run focused tests, then `scripts/dev check`.
