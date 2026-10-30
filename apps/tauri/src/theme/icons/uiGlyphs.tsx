import { glyph } from "./glyphs";

/**
 * The control set: the marks of the chrome, drawn by Forge on the same 16px
 * grid as the domain glyphs in `glyphs.tsx`.
 *
 * The whole icon system — both files — follows these rules, so a mark added
 * later cannot drift:
 *
 * - **One grid, one weight.** Shapes are laid out on the 16px grid and stroked
 *   at 1.5 with round caps and joins; the optical box is roughly 2–14. A glyph
 *   has no fill of its own: `currentColor` is the only ink, which is what lets
 *   the neutral `forge-icon-muted` row and an `forge-icon-accent` selection
 *   tint the same mark. Colour is a class on the caller, never a value here.
 * - **A distinct silhouette, not a stock trace.** These are Forge's own
 *   vocabulary — the hex nut for settings, the half disc for appearance — so
 *   the shell does not read as a borrowed icon set. Generic shapes (chevrons,
 *   a close mark) keep their conventional skeleton on purpose: recognition
 *   beats novelty where the form carries no identity.
 * - **`region` is reserved** for the part that fills at 20% when the mark names
 *   the chosen thing (`.glyph-region` in `base.css`); it is never decoration.
 * - **Drawn for the size it is used at.** Details stay legible at 14px: no
 *   hairlines, no dashes, no fill-dependent states.
 */
export const UI_GLYPHS = {
  // Navigation and window control. Chevrons share one 8×7 span so they align
  // when two sit beside each other.
  arrowLeft: glyph([{ path: "M13.2 8H3.4" }, { path: "M7 4.5 3.5 8l3.5 3.5" }]),
  chevronLeft: glyph([{ path: "M10 4.5 6.4 8 10 11.5" }]),
  chevronRight: glyph([{ path: "M6 4.5 9.6 8 6 11.5" }]),
  chevronUp: glyph([{ path: "M4.5 10 8 6.4 11.5 10" }]),
  chevronDown: glyph([{ path: "M4.5 6 8 9.6 11.5 6" }]),
  close: glyph([{ path: "M4.3 4.3l7.4 7.4" }, { path: "M11.7 4.3l-7.4 7.4" }]),
  plus: glyph([{ path: "M8 3.4v9.2" }, { path: "M3.4 8h9.2" }]),
  minus: glyph([{ path: "M3.4 8h9.2" }]),
  moreHorizontal: glyph([
    { circle: [3.8, 8, 0.75] },
    { circle: [8, 8, 0.75] },
    { circle: [12.2, 8, 0.75] },
  ]),

  // Finding things: the lens is a circle plus a short handle; the funnel is a
  // single closed outline so a chosen filter can fill one region.
  search: glyph([{ circle: [6.9, 6.9, 4.1] }, { path: "M10 10l3.4 3.4" }]),
  filter: glyph([{ path: "M2.8 3.4h10.4l-4 4.7v4.3l-2.4 1.4V8.1z", region: true }]),
  refresh: glyph([
    { path: "M12.9 4.7A5.4 5.4 0 0 0 3.5 6.1" },
    { path: "M13.2 2.7v2.6h-2.6" },
    { path: "M3.1 11.3a5.4 5.4 0 0 0 9.4-1.4" },
    { path: "M2.8 13.3v-2.6h2.6" },
  ]),

  // System and settings: a hex nut (the forge's own hardware) and a half disc
  // (appearance is a light/dark choice).
  settings: glyph([{ path: "M13.5 8l-2.7 4.7H5.2L2.5 8l2.7-4.7h5.6z" }, { circle: [8, 8, 2.1] }]),
  appearance: glyph([
    { circle: [8, 8, 5.3] },
    { path: "M8 2.7a5.3 5.3 0 0 1 0 10.6z", region: true },
  ]),
  stats: glyph([{ path: "M4.4 12.7V8.9" }, { path: "M8 12.7V4.4" }, { path: "M11.6 12.7V7.2" }]),
  loader: glyph([{ path: "M13.5 8A5.5 5.5 0 1 0 8 13.5" }]),

  // Documents and structure.
  copy: glyph([
    {
      path: "M10.2 5.8V4.2a1.6 1.6 0 0 0-1.6-1.6H4.2a1.6 1.6 0 0 0-1.6 1.6v4.4a1.6 1.6 0 0 0 1.6 1.6h1.6",
    },
    { rect: [5.8, 5.8, 7.6, 7.6, 1.7] },
  ]),
  externalLink: glyph([
    {
      path: "M13.4 9.1v2.7a1.6 1.6 0 0 1-1.6 1.6H4.2a1.6 1.6 0 0 1-1.6-1.6V4.2a1.6 1.6 0 0 1 1.6-1.6h2.7",
    },
    { path: "M9.7 2.6h3.7v3.7" },
    { path: "M13.1 2.9 8.8 7.2" },
  ]),
  globe: glyph([
    { path: "M2.4 8a5.6 5.6 0 1 0 11.2 0 5.6 5.6 0 1 0-11.2 0z" },
    { path: "M8 2.4c-1.7 1.6-2.5 3.5-2.5 5.6s.8 4 2.5 5.6c1.7-1.6 2.5-3.5 2.5-5.6S9.7 4 8 2.4z" },
    { path: "M2.6 8h10.8" },
  ]),
  play: glyph([
    { path: "M5 3.4v9.2a.6.6 0 0 0 .9.5l7.3-4.6a.6.6 0 0 0 0-1L5.9 2.9a.6.6 0 0 0-.9.5z" },
  ]),
  stop: glyph([
    { path: "M4.6 3.6h6.8a1 1 0 0 1 1 1v6.8a1 1 0 0 1-1 1H4.6a1 1 0 0 1-1-1V4.6a1 1 0 0 1 1-1z" },
  ]),
  activity: glyph([{ path: "M1.8 8.4h2.6l1.8-4.6 3.4 8.6 1.8-4h2.8" }]),
  edit: glyph([{ path: "M11.3 2.7l2 2-8.4 8.4-2.6.6.6-2.6z" }, { path: "M9.8 4.2l2 2" }]),
  trash: glyph([
    { path: "M2.8 4.2h10.4" },
    {
      path: "M6.2 4.2V3.1a1.1 1.1 0 0 1 1.1-1.1h1.4a1.1 1.1 0 0 1 1.1 1.1v1.1",
    },
    {
      path: "M4.5 4.4l.6 8.1a1.5 1.5 0 0 0 1.5 1.4h2.8a1.5 1.5 0 0 0 1.5-1.4l.6-8.1",
    },
    { path: "M6.9 7v3.6" },
    { path: "M9.1 7v3.6" },
  ]),
  folderOpen: glyph([
    {
      path: "M2.2 11.9V4.4a1.2 1.2 0 0 1 1.2-1.2h2.5l1.4 1.7h4.8a1.2 1.2 0 0 1 1.2 1.2v1.2",
    },
    {
      path: "M2.2 12.6l1.8-4.1a1.3 1.3 0 0 1 1.2-.8h8a1.2 1.2 0 0 1 1.2 1.5l-1.3 3.4a1.3 1.3 0 0 1-1.2.8H3.4a1.3 1.3 0 0 1-1.2-.8z",
    },
  ]),
  file: glyph([
    {
      path: "M9.4 1.9H4.2a1.4 1.4 0 0 0-1.4 1.4v9.4a1.4 1.4 0 0 0 1.4 1.4h7.6a1.4 1.4 0 0 0 1.4-1.4V5.9z",
    },
    { path: "M9.4 1.9v4h3.8" },
  ]),
  image: glyph([
    { rect: [2.4, 3.4, 11.2, 9.2, 1.6] },
    { circle: [5.9, 6.7, 1] },
    { path: "M2.9 11.7l3.1-3.2 2.3 2.4 2.4-2.6 2.9 3" },
  ]),
  imageOff: glyph([
    { rect: [2.4, 3.4, 11.2, 9.2, 1.6] },
    { circle: [5.9, 6.7, 1] },
    { path: "M3.1 12.2l2.5-2.6 1.7 1.8" },
    { path: "M3 3l10 10" },
  ]),

  // Layout.
  maximize: glyph([
    { path: "M9.5 2.6h3.9v3.9" },
    { path: "M13.1 2.9 9 7" },
    { path: "M6.5 13.4H2.6V9.5" },
    { path: "M2.9 13.1 7 9" },
  ]),
  panelLeftOpen: glyph([
    { rect: [1.9, 2.9, 12.2, 10.2, 1.6] },
    { path: "M6.5 2.9v10.2" },
    { path: "M9.4 6.3 11.2 8l-1.8 1.7" },
  ]),
  panelLeftClose: glyph([
    { rect: [1.9, 2.9, 12.2, 10.2, 1.6] },
    { path: "M6.5 2.9v10.2" },
    { path: "M10.6 6.3 8.8 8l1.8 1.7" },
  ]),

  // Agents and decoration. The agent is a head, not a face-in-a-box: it is the
  // fallback identity for a provider with no brand mark.
  agent: glyph([
    { path: "M8 2.9v2.1" },
    { circle: [8, 2.2, 0.65] },
    { rect: [4.3, 5, 7.4, 7.2, 1.8] },
    { circle: [6.6, 8.5, 0.75] },
    { circle: [9.4, 8.5, 0.75] },
    { path: "M6.6 10.7h2.8" },
  ]),
  sparkles: glyph([
    { path: "M7 2.7l1.1 3.2 3.2 1.1-3.2 1.1L7 11.3 5.9 8.1 2.7 7l3.2-1.1z" },
    { path: "M11.9 9.9l.5 1.6 1.6.5-1.6.5-.5 1.6-.5-1.6-1.6-.5 1.6-.5z" },
  ]),
  keyboard: glyph([
    { rect: [1.9, 4.1, 12.2, 7.8, 1.7] },
    { circle: [4.6, 6.5, 0.55] },
    { circle: [7, 6.5, 0.55] },
    { circle: [9.4, 6.5, 0.55] },
    { circle: [11.8, 6.5, 0.55] },
    { path: "M5.5 9.5h5" },
  ]),
} as const;
