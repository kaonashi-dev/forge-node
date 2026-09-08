import { describe, expect, it } from "vitest";
import { contrast, parseHex } from "./mix";
import { SCOPE_CONTRAST_FLOOR, editorPalette } from "./editorTheme";
import { palettes, toCssVariables, type ThemeBaseId } from "./tokens";

/**
 * T2: the shell, the editor and the terminal agree.
 *
 * Three surfaces paint text on a ground, and each derives its colours a
 * different way — the chrome from the semantic tokens, the editor from a
 * `HighlightStyle` built in `editorTheme.ts`, the terminal from the sixteen
 * ANSI slots the renderer reads straight off the root element. Three
 * derivations is three chances for one base to end up with a readable chrome
 * and an unreadable terminal, so this asks the same two questions of all
 * three: are they drawn from *this* base, and does what they draw clear the
 * floor on the ground they draw it on?
 */
const bases = Object.keys(palettes) as ThemeBaseId[];

/** The ANSI brights, which is what a terminal actually paints foreground with. */
const BRIGHTS = [9, 10, 11, 12, 13, 14, 15];

describe.each(bases)("%s", (base) => {
  const palette = palettes[base];
  const variables = toCssVariables(palette);

  it("paints all three surfaces on a ground this base defines", () => {
    expect(variables["--forge-bg"]).toBe(palette.bg);
    expect(variables["--forge-editor"]).toBe(palette.editor);
    expect(variables["--forge-term-bg"]).toBe(palette.termBg);
    expect(editorPalette(base).background).toBe(palette.editor);
  });

  it("holds every syntax scope above the floor on the editor ground", () => {
    const ground = parseHex(palette.editor);
    for (const [scope, color] of Object.entries(editorPalette(base).scopes)) {
      expect(contrast(parseHex(color), ground), `${base}: ${scope}`).toBeGreaterThanOrEqual(
        SCOPE_CONTRAST_FLOOR,
      );
    }
  });

  it("holds the terminal's bright ANSI above the floor on the terminal ground", () => {
    // The brights are what a TUI colours text with; the dims are for fills and
    // are held to nothing here, because nothing sets type in ANSI 0–7 on a
    // ground of the same family.
    const ground = parseHex(palette.termBg);
    for (const index of BRIGHTS) {
      expect(
        contrast(parseHex(palette.ansi[index]), ground),
        `${base}: ansi-${index}`,
      ).toBeGreaterThanOrEqual(SCOPE_CONTRAST_FLOOR);
    }
  });

  it("keeps the terminal's own foreground legible on its own ground", () => {
    expect(contrast(parseHex(palette.termFg), parseHex(palette.termBg))).toBeGreaterThanOrEqual(
      4.5,
    );
  });

  it("emits every variable the terminal renderer reads", () => {
    // `terminal/palette.ts` reads these by name off the root element, with
    // hard-coded fallbacks. A base that failed to emit one would fall back to
    // a gruvbox-hard colour and nothing would say so.
    for (const name of ["--forge-term-fg", "--forge-term-bg", "--forge-accent", "--forge-step"]) {
      expect(variables[name], `${base}: ${name}`).toMatch(/^#[0-9a-f]{6}$/);
    }
    for (let index = 0; index < 16; index += 1) {
      expect(variables[`--forge-ansi-${index}`], `${base}: ansi-${index}`).toMatch(
        /^#[0-9a-f]{6}$/,
      );
    }
  });
});
