import { describe, expect, it } from "vitest";
import { contrast, mix, parseHex, pct } from "../theme/mix";
import { palettes, staticTokens, toCssVariables, type ThemeBaseId } from "../theme/tokens";
import kit from "./ui.css?raw";

const AA = 4.5;
const bases = Object.keys(palettes) as ThemeBaseId[];

/** The percentage a `--*-tint-bg` token mixes in, read off the token itself. */
function tint(token: string): number {
  const found = staticTokens()[token]?.match(/ (\d+)%, transparent\)$/);
  if (!found) throw new Error(`${token} is not a transparent tint`);
  return Number(found[1]);
}

/**
 * Text the kit sets on a translucent tint. `color-mix(…, transparent)` lands on
 * whatever is underneath, so each pairing is checked over every ground a
 * control sits on: the panel, a raised surface and the sidebar.
 */
describe("kit tints keep text at AA", () => {
  it("still uses the percentages checked below", () => {
    expect(kit).toContain("var(--badge-hue) 12%, transparent");
    expect(kit).toContain("var(--danger-solid) 12%, transparent");
    expect(kit).toContain("var(--attention) 20%, transparent");
  });

  it.each(bases)("%s", (base) => {
    const p = palettes[base];
    const v = toCssVariables(p);
    const text = (token: string) => parseHex(v[token]);
    const over = (ground: string, hue: string, percent: number) =>
      mix(parseHex(ground), parseHex(hue), pct(percent));
    const check = (label: string, fg: number, bg: number) =>
      expect(contrast(fg, bg), `${base}: ${label}`).toBeGreaterThanOrEqual(AA);

    for (const ground of [p.bg, p.surface, p.sidebar]) {
      check(`text on --select-bg over ${ground}`, text("--fg-default"), over(ground, p.accent, 13));
      check(
        `text on --attention-bg over ${ground}`,
        text("--fg-default"),
        over(ground, p.needsYou, 13),
      );
      for (const [role, hue] of Object.entries({
        accent: p.accent,
        success: p.green,
        warning: p.amber,
        danger: p.red,
        attention: p.needsYou,
      })) {
        check(`${role} badge over ${ground}`, text(`--${role}-text`), over(ground, hue, 12));
      }
      for (const percent of [tint("--danger-tint-bg"), 12]) {
        check(
          `danger-ghost ${percent}% over ${ground}`,
          text("--danger-text"),
          over(ground, p.red, percent),
        );
      }
      for (const percent of [13, 20]) {
        check(
          `attention-soft ${percent}% over ${ground}`,
          text("--attention-text"),
          over(ground, p.needsYou, percent),
        );
      }
      check(
        `accent callout over ${ground}`,
        text("--accent-text"),
        over(ground, p.accent, tint("--accent-tint-bg")),
      );
      check(
        `attention callout over ${ground}`,
        text("--fg-default"),
        over(ground, p.needsYou, tint("--attention-tint-bg")),
      );
    }

    const dangerToast = over(p.surface, p.red, tint("--danger-tint-bg"));
    check("danger toast detail", text("--fg-default"), dangerToast);
    check("danger toast icon text", text("--danger-text"), dangerToast);
    check("invalid field text", text("--fg-default"), dangerToast);
  });
});
