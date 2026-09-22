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

/** Preserve the hue where possible; incompatible custom grounds get the best endpoint. */
export function readableColor(
  color: number,
  grounds: readonly number[],
  toward: number,
  floor = 4.5,
): number {
  const score = (candidate: number) =>
    Math.min(...grounds.map((ground) => contrast(candidate, ground)));
  if (score(color) >= floor) return color;
  let target = toward;
  if (score(target) < floor) {
    for (const candidate of [0, 0xffffff]) {
      if (score(candidate) > score(target)) target = candidate;
    }
  }
  for (let step = 1; step <= 25; step += 1) {
    const candidate = mix(color, target, pct(step * 4));
    if (score(candidate) >= floor) return candidate;
  }
  return score(target) > score(color) ? target : color;
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

// Diff washes use the editor ground so they do not introduce seams in a patch.
export function derivedTokens(args: {
  bg: number;
  text: number;
  sidebar: number;
  editor: number;
  amber: number;
  needsYou: number;
  gitAdded: number;
  gitDeleted: number;
}): Record<string, string> {
  const { bg, text, sidebar, editor, amber, needsYou, gitAdded, gitDeleted } = args;
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
  };
}
