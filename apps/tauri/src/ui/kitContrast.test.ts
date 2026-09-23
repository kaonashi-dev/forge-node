import { describe, expect, it } from "vitest";
import { contrast, mix, parseHex, pct } from "../theme/mix";
import { palettes, toCssVariables, type ThemeBaseId } from "../theme/tokens";
import kit from "./ui.css?raw";
import workbench from "../styles/workbench.css?raw";

const AA = 4.5;
const bases = Object.keys(palettes) as ThemeBaseId[];

/**
 * Text the kit sets on a translucent tint. `color-mix(…, transparent)` lands on
 * whatever is underneath, so each pairing is checked over every ground a
 * control sits on: the panel, a raised surface and the sidebar.
 */
describe("kit tints keep text at AA", () => {
  it("still uses the percentages checked below", () => {
    expect(kit).toContain("var(--badge-hue) 10%, transparent");
    expect(kit).toContain("var(--danger-solid) 8%, transparent");
    expect(kit).toContain("var(--danger-solid) 12%, transparent");
    expect(kit).toContain("var(--danger-solid) 7%, var(--surface)");
    expect(kit).toContain("var(--danger-solid) 5%, var(--surface)");
    expect(workbench).toContain("var(--accent) 6%, transparent");
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
        check(`${role} badge over ${ground}`, text(`--${role}-text`), over(ground, hue, 10));
      }
      for (const percent of [8, 12]) {
        check(
          `danger-ghost ${percent}% over ${ground}`,
          text("--danger-text"),
          over(ground, p.red, percent),
        );
      }
      check(`markdown callout over ${ground}`, text("--accent-text"), over(ground, p.accent, 6));
    }

    const dangerToast = over(p.surface, p.red, 7);
    check("danger toast detail", text("--fg-muted"), dangerToast);
    check("danger toast icon text", text("--danger-text"), dangerToast);
    check("invalid field text", text("--fg-default"), over(p.surface, p.red, 5));
  });
});
