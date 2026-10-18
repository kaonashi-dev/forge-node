import { hex, luminance, mix, parseHex, pct } from "./mix";
import { deriveStrip, type Palette } from "./tokens";

/** How far the window ground sinks toward black so the frosted panes lift off it. */
const WINDOW_SINK = 35;

/**
 * The Glass finish over a dark base. It is a palette transform rather than a
 * theme of its own: the base keeps its card, text, accent and terminal colours,
 * and only the window behind the panes sinks, so each base frosts in its own
 * hues and switching Glass off is applying the base again. The glowing frame,
 * the sidebar's pane and the pill tabs are structure, and live in `shell.css`
 * under `[data-glass]`.
 */
export function withGlass(source: Palette): Palette {
  const bg = parseHex(source.bg);
  const darkest = [parseHex(source.rail), bg].reduce((a, b) =>
    luminance(b) < luminance(a) ? b : a,
  );
  const window = hex(mix(darkest, 0, pct(WINDOW_SINK)));
  return {
    ...source,
    ansi: [...source.ansi],
    rail: window,
    sidebar: window,
    strip: deriveStrip(window, source.bg),
  };
}
