import { describe, expect, it } from "vitest";
import { syntaxVariables } from "./editorTheme";
import { palettes, staticTokens, toCssVariables } from "./tokens";
import generated from "./tokens.css?raw";
import base from "../styles/base.css?raw";
import shell from "../styles/shell.css?raw";
import workbench from "../styles/workbench.css?raw";
import panels from "../styles/panels.css?raw";
import settings from "../styles/settings.css?raw";
import fileSurface from "../features/files/explorer/fileSurface.css?raw";
import components from "../ui/ui.css?raw";
import gallery from "../app/gallery.css?raw";
import fileWorkbench from "../../packages/file-workbench/src/style.css?raw";

/**
 * Reject styles that bypass theme tokens.
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
  ["features/files/explorer/fileSurface.css", fileSurface],
  ["app/gallery.css", gallery],
  ["ui/ui.css", components],
];
const KIT = "ui/ui.css";
const BASE = "styles/base.css";

const SOURCES = import.meta.glob(["../**/*.{ts,tsx}", "!../**/*.test.{ts,tsx}"], {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

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

type Declaration = Offence & {
  /** The innermost style rule's selector. */
  selector: string;
  /** Every enclosing at-rule prelude, outermost first. */
  at: string[];
  property: string;
  value: string;
};

/**
 * Every declaration in every sheet, with the rule and at-rules around it.
 *
 * A line scan cannot answer "is this inside `@font-face`" or "which selector
 * is this opacity for", and those are the questions the rules below ask.
 */
function declarations(): Declaration[] {
  const found: Declaration[] = [];
  for (const [sheet, source] of SHEETS) {
    const stack: string[] = [];
    let pending = "";
    let line = 1;
    for (let index = 0; index < source.length; index += 1) {
      if (source.startsWith("/*", index)) {
        const end = source.indexOf("*/", index + 2);
        line += source.slice(index, end).split("\n").length - 1;
        index = end + 1;
        continue;
      }
      const char = source[index];
      if (char === "\n") line += 1;
      if (char === "{") {
        stack.push(pending.trim());
        pending = "";
      } else if (char === "}") {
        stack.pop();
        pending = "";
      } else if (char === ";") {
        const text = pending.trim();
        const colon = text.indexOf(":");
        const at = stack.filter((prelude) => prelude.startsWith("@"));
        const selector = [...stack].reverse().find((prelude) => !prelude.startsWith("@")) ?? "";
        if (colon > 0 && stack.length > 0) {
          found.push({
            sheet,
            line,
            text,
            selector,
            at,
            property: text.slice(0, colon).trim(),
            value: text.slice(colon + 1).trim(),
          });
        }
        pending = "";
      } else {
        pending += char;
      }
    }
  }
  return found;
}

function offending(test: (declaration: Declaration) => boolean): Offence[] {
  return declarations()
    .filter(test)
    .map(({ sheet, line, text }) => ({ sheet, line, text }));
}

/** Whether `needle` sits inside an unclosed top-level `@media` block that gates on `condition`. */
function insideMedia(source: string, needle: string, condition: string): boolean {
  const lines = source.slice(0, source.indexOf(needle)).split("\n");
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    if (lines[index] === "}") return false;
    if (lines[index].startsWith("@media")) return lines[index].includes(condition);
  }
  return false;
}

describe("stylesheet motion", () => {
  it("keeps the two looping placeholders off when the OS asks for less motion", () => {
    for (const name of ["forge-progress-shuttle 1.2s", "forge-skeleton-pulse 1.6s"]) {
      expect(insideMedia(components, `animation: ${name}`, "no-preference"), name).toBe(true);
    }
  });
});

describe("stylesheet token discipline", () => {
  it("inspects real CSS instead of the test runner's empty style mocks", () => {
    for (const [name, source] of SHEETS) expect(source, name).toContain("{");
    expect(declarations().filter(({ at }) => at.length > 0).length).toBeGreaterThan(20);
    expect(Object.keys(SOURCES).length).toBeGreaterThan(100);
  });

  it("includes every built-in theme before runtime initialization", () => {
    for (const base of Object.keys(palettes)) expect(generated).toContain(`[data-theme="${base}"]`);
  });

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
    const masked = /mask-image|--stripe|linear-gradient\(\s*$|#000 (?:calc\(|var\(--space-|\d)/;
    expect(scan((line) => /#[0-9a-fA-F]{3,8}\b/.test(line) && !masked.test(line))).toEqual([]);
  });

  it("names the semantic layer wherever one exists", () => {
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

  it("uses readable foregrounds instead of solid fills for text", () => {
    expect(
      scan((line) =>
        /^\s*color: var\(--(?:accent|danger|success|warning|attention|info)-solid\)/.test(line),
      ),
    ).toEqual([]);
  });

  it("keeps button variants owned by the control kit", () => {
    const overrides = SHEETS.filter(([name]) => name !== "ui/ui.css")
      .filter(([, source]) => /\.forge-btn-(?:primary|secondary|danger|ghost)\b/.test(source))
      .map(([name]) => name);
    expect(overrides).toEqual([]);
  });

  it("references only variables something defines", () => {
    // A `var()` with a fallback may be absent by design; one without resolves
    // to nothing when its name is wrong, and the property silently drops.
    const defined = new Set([
      ...Object.keys(staticTokens()),
      ...Object.keys(toCssVariables(palettes["gruvbox-hard"])),
      ...Object.keys(syntaxVariables("gruvbox-hard")),
      ...declarations()
        .filter(({ property }) => property.startsWith("--"))
        .map(({ property }) => property),
      ...[...fileWorkbench.matchAll(/(--fw-[\w-]+)\s*:/g)].map((match) => match[1]),
      ...Object.values(SOURCES).flatMap((source) =>
        [...source.matchAll(/["'`](--[a-z][\w-]*)["'`]/g)].map((match) => match[1]),
      ),
    ]);
    expect(
      offending(({ value }) =>
        [...value.matchAll(/var\((--[\w-]+)\)/g)].some(
          ([, name]) => !defined.has(name) && !name.startsWith("--kb-"),
        ),
      ),
    ).toEqual([]);
  });
});

describe("stylesheet type, colour and motion discipline", () => {
  const scaled = /^(var\(--[\w-]+\)|0|inherit|normal)$/;

  it("weights, leads and tracks type off the scale", () => {
    expect(
      offending(
        ({ property, value, at }) =>
          ["font-weight", "line-height", "letter-spacing"].includes(property) &&
          !scaled.test(value) &&
          !at.some((prelude) => prelude.startsWith("@font-face")),
      ),
    ).toEqual([]);
  });

  it("recedes every disabled state by the one opacity", () => {
    expect(
      offending(
        ({ property, value, selector }) =>
          property === "opacity" &&
          /disabled/.test(selector) &&
          value !== "var(--opacity-disabled)",
      ),
    ).toEqual([]);
  });

  it("mixes colours only in the kit, everywhere else through a token", () => {
    expect(offending(({ sheet, value }) => sheet !== KIT && value.includes("color-mix("))).toEqual(
      [],
    );
  });

  it("names no colour as rgb or hsl", () => {
    expect(offending(({ value }) => /\b(?:rgb|hsl)a?\(/.test(value))).toEqual([]);
  });

  it("truncates one line through `.forge-truncate`", () => {
    // The kit's own components keep their recipe; a feature adds the class.
    expect(
      offending(
        ({ sheet, property, value }) =>
          sheet !== KIT && sheet !== BASE && property === "text-overflow" && value === "ellipsis",
      ),
    ).toEqual([]);
  });

  it("draws a focus ring only from the focus tokens", () => {
    expect(
      offending(
        ({ property, value }) =>
          value.includes("var(--ring)") &&
          !value.includes("--focus-ring") &&
          property !== "border-color",
      ),
    ).toEqual([]);
  });

  it("animates nothing the OS asked to keep still", () => {
    expect(
      offending(
        ({ property, value, at }) =>
          property === "transition" &&
          !value.startsWith("none") &&
          !at.some((prelude) => prelude.includes("prefers-reduced-motion: no-preference")),
      ),
    ).toEqual([]);
  });

  it("leaves a kit control's paint to the kit", () => {
    const control =
      /\.forge-(?:btn|field|switch|badge|checkbox|textarea|select-trigger|segment|chip|kbd)\b[^\s>+~]*$/;
    const paint =
      /^(?:background(?:-color)?|border(?:-color)?|color|box-shadow|border-radius|font-weight)$/;
    expect(
      offending(
        ({ sheet, selector, property }) =>
          sheet !== KIT &&
          paint.test(property) &&
          selector.split(",").some((part) => control.test(part.trim())),
      ),
    ).toEqual([]);
  });
});

describe("component markup discipline", () => {
  const tsx = Object.entries(SOURCES).filter(([path]) => path.endsWith(".tsx"));

  it("keeps constant styles in a stylesheet", () => {
    // A value computed at runtime, or a custom property carrying one, is what
    // `style` is for; a literal belongs to a class that a theme can reach.
    const literal =
      /(?:^|[{,])\s*("?[\w-]+"?)\s*:\s*("[^"$`]*"|'[^']*'|-?\d+(?:\.\d+)?)\s*(?=,|$)/g;
    const found = tsx.flatMap(([path, source]) =>
      [...source.matchAll(/style=\{\{([\s\S]*?)\}\}/g)]
        .filter(([, body]) => [...body.trim().matchAll(literal)].length > 0)
        .map(([text]) => ({ path, text })),
    );
    expect(found).toEqual([]);
  });

  it("renders only classes the kit or the base layer defines from `ui/`", () => {
    const defined = new Set(
      declarations()
        .filter(({ sheet }) => sheet === KIT || sheet === BASE)
        .flatMap(({ selector }) => [...selector.matchAll(/\.([a-zA-Z][\w-]*)/g)].map((m) => m[1])),
    );
    const found = tsx
      .filter(([path]) => path.startsWith("../ui/"))
      .flatMap(([path, source]) =>
        [...source.matchAll(/class=(?:"([^"]*)"|\{`([^`]*)`\})/g)].flatMap((match) =>
          (match[1] ?? match[2] ?? "")
            .replace(/\$\{[^}]*\}/g, " ")
            .split(/\s+/)
            .filter((name) => /^[a-z][\w-]*[a-z0-9]$/.test(name) && !defined.has(name))
            .map((name) => ({ path, name })),
        ),
      );
    expect(found).toEqual([]);
  });
});
