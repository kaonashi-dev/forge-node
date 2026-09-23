import { describe, expect, it } from "vitest";
import { contrast, hex, mix, parseHex, pct } from "./mix";
import { withThemeContrast } from "./themeVariants";
import { metrics, palettes, scale, staticTokens, toCssVariables, type ThemeBaseId } from "./tokens";

/**
 * Internal coherence of the design scale.
 *
 * This is what replaced the geometry half of `tokens.test.ts` when the Rust
 * `theme tokens` went away with the GUI: the density scale has
 * no external contract to be pinned to any more, so it is guarded for the
 * properties a scale must have on its own terms — monotone ladders and AA
 * contrast — rather than for equality with a frozen list of pixels. A denser
 * ladder (F2) then changes the numbers without touching a single assertion.
 */

const bases = Object.keys(palettes) as ThemeBaseId[];

/** Every ladder is strictly ascending, low rung to high. */
function ascending(values: number[]): boolean {
  return values.every((v, i) => i === 0 || v > values[i - 1]);
}

describe("scale coherence", () => {
  it("keeps the control-height ladder ascending", () => {
    expect(
      ascending([metrics.controlXS, metrics.controlSM, metrics.controlMD, metrics.controlLG]),
    ).toBe(true);
  });

  it("keeps the radius ladder ascending, full at the top", () => {
    expect(
      ascending([
        metrics.radiusXS,
        metrics.radiusSM,
        metrics.radiusMD,
        metrics.radiusLG,
        metrics.radiusXL,
      ]),
    ).toBe(true);
  });

  it("keeps the type scale ascending", () => {
    expect(
      ascending([metrics.textXS, metrics.textSM, metrics.textMD, metrics.textLG, metrics.textXL]),
    ).toBe(true);
  });
});

describe("AA contrast", () => {
  const AA = 4.5;
  const c = (a: string, b: string) => contrast(parseHex(a), parseHex(b));

  it("holds primary text at AA on every surface, in every theme", () => {
    for (const base of bases) {
      const p = palettes[base];
      for (const ground of [p.bg, p.surface, p.surfaceHi, p.sidebar, p.rail, p.editor]) {
        expect(c(p.text, ground), `${base}: text on ${ground}`).toBeGreaterThanOrEqual(AA);
      }
    }
  });

  it.each(bases)("keeps %s readable across every contrast setting", (base) => {
    for (let level = 0; level <= 100; level += 1) {
      const p = withThemeContrast(palettes[base], level);
      const v = toCssVariables(p);
      const grounds = [
        p.bg,
        p.surface,
        p.surfaceHi,
        p.sidebar,
        p.rail,
        p.editor,
        ...["hover", "selected", "pressed", "sidebar-hi", "sidebar-pressed", "needs-you-tint"].map(
          (name) => v[`--forge-${name}`],
        ),
        v["--accent-soft"],
        v["--danger-soft"],
        ...[p.bg, p.sidebar, p.surface, p.surfaceHi].flatMap((ground) =>
          [p.accent, p.needsYou].map((tint) => hex(mix(parseHex(ground), parseHex(tint), pct(13)))),
        ),
      ];
      for (const token of [
        "--fg-default",
        "--fg-muted",
        "--fg-subtle",
        "--accent-text",
        "--danger-text",
        "--success-text",
        "--warning-text",
        "--attention-text",
        "--info-text",
        "--git-added-text",
        "--git-modified-text",
        "--git-deleted-text",
        "--git-untracked-text",
        "--git-conflict-text",
        "--git-ignored-text",
        "--forge-focus-ring",
      ]) {
        const minimum = Math.min(...grounds.map((ground) => c(v[token], ground)));
        expect(minimum, `${base} contrast ${level}: ${token}`).toBeGreaterThanOrEqual(
          token === "--forge-focus-ring" ? 3 : AA,
        );
      }
      for (const [role, fill] of Object.entries({
        accent: p.accent,
        danger: p.red,
        attention: p.needsYou,
        success: p.green,
        warning: p.amber,
        info: p.blue,
      })) {
        expect(c(v[`--${role}-fg`], fill), `${base}: ${role} fill`).toBeGreaterThanOrEqual(AA);
      }
      for (const role of ["accent", "danger"]) {
        expect(
          c(v[`--${role}-hover-fg`], v[`--${role}-solid-hover`]),
          `${base}: ${role} hover`,
        ).toBeGreaterThanOrEqual(AA);
      }
    }
  });
});

/**
 * The semantic layer and its `--forge-*` aliases.
 *
 * Every legacy name the stylesheets still read has to resolve to *something* —
 * transitively, since the forwards chain — and to the same value it carried
 * before the layer existed, or F1 would have quietly recoloured the shell it
 * was meant to leave untouched.
 */
describe("semantic layer", () => {
  const merged = (base: ThemeBaseId): Map<string, string> =>
    new Map(Object.entries({ ...staticTokens(), ...toCssVariables(palettes[base]) }));

  /** Follow `var(--x)` forwards to the concrete value, refusing cycles. */
  function resolve(name: string, map: Map<string, string>, seen = new Set<string>()): string {
    if (seen.has(name)) throw new Error(`token cycle at ${name}`);
    seen.add(name);
    const raw = map.get(name);
    if (raw === undefined) throw new Error(`undefined token ${name}`);
    const forward = raw.match(/^var\((--[a-z0-9-]+)\)$/);
    return forward ? resolve(forward[1], map, seen) : raw;
  }

  it("keeps the spacing scale ascending", () => {
    expect(scale.space.every((v, i) => i === 0 || v > scale.space[i - 1])).toBe(true);
  });

  it("resolves every static forward to a defined token, in every base", () => {
    for (const base of bases) {
      const map = merged(base);
      for (const [name, value] of Object.entries(staticTokens())) {
        if (value.startsWith("var(")) {
          expect(() => resolve(name, map), `${base}: ${name}`).not.toThrow();
        }
      }
    }
  });

  it("preserves the pre-layer value behind every colour alias", () => {
    for (const base of bases) {
      const p = palettes[base];
      const map = merged(base);
      const same: [string, string][] = [
        ["--bg-base", p.bg],
        ["--bg-subtle", p.sidebar],
        ["--bg-raised", p.surface],
        ["--bg-overlay", p.surfaceHi],
        ["--accent-solid", p.accent],
        ["--danger-solid", p.red],
        ["--success-solid", p.green],
        ["--warning-solid", p.amber],
        ["--attention-solid", p.needsYou],
      ];
      for (const [token, expected] of same) {
        expect(resolve(token, map), `${base}: ${token}`).toBe(expected);
      }
    }
  });

  it("keeps geometry, elevation and derived foregrounds intact", () => {
    const map = merged("forge-dark");
    expect(resolve("--control-md", map)).toBe(`${metrics.controlMD}px`);
    expect(resolve("--radius-xs", map)).toBe(`${metrics.radiusXS}px`);
    expect(resolve("--forge-shadow-md", map)).toBe("0 16px 34px rgb(0 0 0 / 40%)");
    expect(resolve("--forge-motion-fast", map)).toBe("90ms");
    for (const token of ["--accent-fg", "--danger-fg", "--attention-fg", "--border-strong"]) {
      expect(resolve(token, map), token).toMatch(/^#[0-9a-f]{6}$/);
    }
  });
});
