import { metrics } from "./tokens";

/**
 * The Trim finish: the same chrome squared off and pulled in.
 *
 * These are inline overlays rather than stylesheet defaults because
 * `applyTheme` rewrites every `--forge-*` metric; `ThemeProvider` reapplies
 * them after a theme switch, the same contract density has.
 *
 * Terminal and editor padding stay out: their panes subtract `metrics.*Pad`
 * when they measure the cell grid, so a CSS-only override would size the
 * canvas against padding the pane is not actually drawing.
 */
const CHROME: Record<string, number> = {
  "--forge-strip-h": metrics.stripH - 6,
  "--forge-tab-h": metrics.tabH - 4,
  "--forge-location-h": metrics.locationH - 4,
  "--forge-open-files-h": metrics.openFilesH - 4,
};

/**
 * Only the rungs: badge, row, control, card and panel forward from them, so
 * one ladder holds. `--forge-radius-xl` is deliberately absent because
 * `--radius-full` derives every circle from it.
 */
const RADII: Record<string, number> = {
  "--forge-radius-xs": 2,
  "--forge-radius-sm": 3,
  "--forge-radius-md": 4,
  "--forge-radius-lg": 6,
};

/** Fixed by `staticTokens` rather than forwarded from a rung. */
const STATIC_RADII: Record<string, string> = {
  "--radius-2xs": "1px",
  "--radius-list": "4px",
  "--radius-palette": "6px",
  "--radius-pill": "4px",
};

/** The variables the finish writes. Pure, so a test can read them. */
export function trimTokens(): Record<string, string> {
  const out: Record<string, string> = { ...STATIC_RADII };
  for (const [name, value] of Object.entries(CHROME)) out[name] = `${value}px`;
  for (const [name, value] of Object.entries(RADII)) out[name] = `${value}px`;
  return out;
}

/**
 * Call after `applyTheme`: the base's metric overrides must already be on the
 * root, because turning the finish off removes only the static radii here and
 * lets the freshly applied theme own the rest.
 */
export function applyTrimChrome(on: boolean): void {
  const root = document.documentElement;
  if (on) root.dataset.trim = "true";
  else delete root.dataset.trim;
  for (const name of Object.keys(STATIC_RADII)) root.style.removeProperty(name);
  if (!on) return;
  for (const [name, value] of Object.entries(trimTokens())) {
    root.style.setProperty(name, value);
  }
}
