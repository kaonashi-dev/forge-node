// Port of `theme tokens::{mix, pct, luminance, contrast, on}`.
// Derived interaction colors must match tokens.ts, not CSS `color-mix`.

export function mix(base: number, over: number, amount: number): number {
  const overWeight = amount & 0xff;
  const baseWeight = 255 - overWeight;
  const channel = (shift: number) =>
    ((((base >> shift) & 0xff) * baseWeight + ((over >> shift) & 0xff) * overWeight + 127) / 255) |
    0;
  return (channel(16) << 16) | (channel(8) << 8) | channel(0);
}

export function pct(percent: number): number {
  return (((percent * 255 + 50) / 100) | 0) & 0xff;
}

function srgbChannel(byte: number): number {
  const value = byte / 255;
  return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
}

export function luminance(color: number): number {
  return (
    0.2126 * srgbChannel((color >> 16) & 0xff) +
    0.7152 * srgbChannel((color >> 8) & 0xff) +
    0.0722 * srgbChannel(color & 0xff)
  );
}

export function contrast(a: number, b: number): number {
  const la = luminance(a);
  const lb = luminance(b);
  const high = Math.max(la, lb);
  const low = Math.min(la, lb);
  return (high + 0.05) / (low + 0.05);
}

export function on(fill: number, bg: number, text: number): number {
  return contrast(text, fill) >= contrast(bg, fill) ? text : bg;
}

export function hex(color: number): string {
  return `#${color.toString(16).padStart(6, "0")}`;
}

export function parseHex(value: string): number {
  const raw = value.replace("#", "");
  if (!/^[0-9a-fA-F]{6}$/.test(raw)) {
    throw new Error(`invalid hex color: ${value}`);
  }
  return Number.parseInt(raw, 16);
}

/**
 * Alpha of the focus halo (`theme::RING_ALPHA`).
 *
 * The ring is `accent` at partial strength so it reads as a wash around a
 * control rather than as a second control. It is the one derived token that
 * stays translucent: an outline is painted over whatever the control sits on,
 * and a colour mixed against one ground would be wrong on every other.
 */
const RING_ALPHA = 0.35;

/**
 * Every interaction colour, derived the way `tokens.ts` derives them.
 *
 * The formulas are copied, not approximated — including *which ground* each
 * one mixes into. The two diff washes go into `editor` and not `bg` because
 * the patch pane is an editor surface, and a wash mixed into the wrong ground
 * reads as a seam down the middle of the file.
 */
export function derivedTokens(args: {
  bg: number;
  text: number;
  sidebar: number;
  editor: number;
  accent: number;
  amber: number;
  needsYou: number;
  gitAdded: number;
  gitDeleted: number;
}): Record<string, string> {
  const { bg, text, sidebar, editor, accent, amber, needsYou, gitAdded, gitDeleted } = args;
  return {
    "--forge-border": hex(mix(bg, text, pct(7))),
    "--forge-border-hi": hex(mix(bg, text, pct(15))),
    "--forge-hover": hex(mix(bg, text, pct(6))),
    "--forge-selected": hex(mix(bg, text, pct(14))),
    "--forge-pressed": hex(mix(bg, text, pct(20))),
    "--forge-sidebar-hi": hex(mix(sidebar, text, pct(6))),
    "--forge-sidebar-pressed": hex(mix(sidebar, text, pct(14))),
    "--forge-diff-added-bg": hex(mix(editor, gitAdded, pct(14))),
    "--forge-diff-removed-bg": hex(mix(editor, gitDeleted, pct(14))),
    "--forge-needs-you-tint": hex(mix(bg, needsYou, pct(14))),
    "--forge-activity-tint": hex(mix(bg, amber, pct(12))),
    "--forge-focus-ring": rgba(accent, RING_ALPHA),
  };
}

/** `0x6cacbd`, 0.35 → `rgb(108 172 189 / 0.35)`. */
export function rgba(color: number, alpha: number): string {
  const channel = (shift: number) => (color >> shift) & 0xff;
  return `rgb(${channel(16)} ${channel(8)} ${channel(0)} / ${alpha})`;
}
