import { describe, expect, it } from "vitest";
import { staticTokens } from "./tokens";
import base from "../styles/base.css?raw";
import shell from "../styles/shell.css?raw";
import workbench from "../styles/workbench.css?raw";
import panels from "../styles/panels.css?raw";
import settings from "../styles/settings.css?raw";
import harness from "../styles/harness.css?raw";
import late from "../styles/late.css?raw";
import components from "../ui/ui.css?raw";

/**
 * The gate that keeps the token ladders load-bearing (`plan-ui-ux.md` §5.1).
 *
 * The stylesheets held 221 raw spacing values, seven raw `z-index` integers
 * and ten radius literals before this ran, and every one of them was a place
 * the scale and the screen could disagree without anything failing. The rules
 * below are deliberately property-scoped rather than a blanket ban on `px`: a
 * hairline border, a fixed panel width and a `max-width` on a column of prose
 * are not spacing, have no rung, and inventing one for them would be the
 * opposite of a scale.
 *
 * Nothing here is a baseline to ratchet down. The migration is finished, so
 * the expected count is zero and a new occurrence fails on the line it is on.
 */
const SHEETS: ReadonlyArray<readonly [name: string, source: string]> = [
  ["styles/base.css", base],
  ["styles/shell.css", shell],
  ["styles/workbench.css", workbench],
  ["styles/panels.css", panels],
  ["styles/settings.css", settings],
  ["styles/harness.css", harness],
  ["styles/late.css", late],
  ["ui/ui.css", components],
];

/** Properties whose values are spacing and must come off `--space-*`. */
const SPACING =
  /^ *(gap|row-gap|column-gap|padding|padding-(top|right|bottom|left|inline|block)(-(start|end))?|margin|margin-(top|right|bottom|left|inline|block)(-(start|end))?|inset): [^;]*?\d+px/;

type Offence = { sheet: string; line: number; text: string };

function scan(test: (line: string) => boolean): Offence[] {
  const found: Offence[] = [];
  for (const [sheet, source] of SHEETS) {
    source.split("\n").forEach((text, index) => {
      if (test(text)) found.push({ sheet, line: index + 1, text: text.trim() });
    });
  }
  return found;
}

describe("stylesheet token discipline", () => {
  it("spends no raw pixels on spacing", () => {
    expect(scan((line) => SPACING.test(line))).toEqual([]);
  });

  it("stacks nothing on a raw integer", () => {
    expect(scan((line) => /^ *z-index: *-?\d/.test(line))).toEqual([]);
  });

  it("rounds every corner off the radius ladder", () => {
    // `50%` is a circle, not a radius: it tracks the box and no rung can.
    expect(scan((line) => /^ *border-radius:[^;]*\d+px/.test(line))).toEqual([]);
  });

  it("sizes every glyph off the type scale", () => {
    expect(scan((line) => /^ *font-size: *\d/.test(line))).toEqual([]);
  });

  it("names no colour as a hex outside a mask", () => {
    // The survivors are `#000` in `mask-image` gradients and the striped
    // "in progress" fill, where the value is an alpha channel rather than a
    // colour and reading a theme token there would be a category error.
    const masked = /mask-image|--stripe|linear-gradient\(\s*$|#000 (calc\()?\d/;
    expect(scan((line) => /#[0-9a-fA-F]{3,8}\b/.test(line) && !masked.test(line))).toEqual([]);
  });

  it("names the semantic layer wherever one exists (§5.1 S3)", () => {
    // `--forge-*` is the generated layer the palettes and metrics emit, and it
    // is not going away: `theme/mix.ts` is a documented mix formula and
    // several of its names — `--forge-git-added`, `--forge-term-bg`,
    // `--forge-title-h` — are the only name that value has. What must not come
    // back is a rule naming the legacy spelling of something the semantic layer
    // already covers: two names for one colour is two places to change it.
    const forwarded = Object.entries(staticTokens())
      .filter(([, value]) => /^var\(--forge-[a-z0-9-]+\)$/.test(value))
      .map(([, value]) => value.slice(4, -1));
    const legacy = new RegExp(
      `var\\((${forwarded.map((n) => n.replace(/-/g, "\\-")).join("|")})\\)`,
    );
    expect(scan((line) => legacy.test(line))).toEqual([]);
  });

  it("repeats no token value as an inline fallback", () => {
    // `var(--forge-text-sm, 12px)` is a second copy of the scale that no test
    // and no theme switch can reach. Scoped to variables this app defines:
    // Kobalte writes `--kb-*` at runtime, and those genuinely can be absent,
    // so a literal fallback there is the only way to have a first frame.
    const own =
      /var\(--(forge|space|text|radius|z|control|row|bg|fg|border|accent|danger|success|warning|attention|ring|shadow|motion|ease|font)[a-z0-9-]*, *[^)]*\d+px\)/;
    expect(scan((line) => own.test(line))).toEqual([]);
  });
});
