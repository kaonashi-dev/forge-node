import { describe, expect, it } from "vitest";
import contract from "../../tests/fixtures/theme.json";
import { palettes, toCssVariables, type Palette, type ThemeBaseId } from "./tokens";

/**
 * The colour contract lives in `tests/fixtures/theme.json`, mirrored here in
 * TypeScript. These tests ensure the TS side still matches that JSON fixture.
 *
 * The fixture used to carry the geometry too, paired by hand against the Rust
 * `theme tokens`. That crate is gone with the GUI, so the
 * geometry has no second owner to stay in step with: the Tauri side owns the
 * density scale outright, and `ui.test.ts` guards it for internal coherence
 * (monotonic ladders, resolvable aliases, AA contrast) instead of for parity
 * with a contract that no longer exists.
 */
describe("theme parity with fixture", () => {
  it("declares exactly the bases the fixture does", () => {
    expect(Object.keys(palettes).sort()).toEqual([...contract.bases].sort());
  });

  it("transcribes every literal colour of every base", () => {
    for (const base of contract.bases as ThemeBaseId[]) {
      const expected = contract.palettes[base as keyof typeof contract.palettes].palette;
      const actual = palettes[base] as unknown as Record<string, unknown>;
      for (const [token, value] of Object.entries(expected)) {
        expect(actual[token], `${base}.${token}`).toEqual(value);
      }
    }
  });

  it("carries all sixteen ANSI colours, in order", () => {
    for (const base of contract.bases as ThemeBaseId[]) {
      const expected = contract.palettes[base as keyof typeof contract.palettes].palette.ansi;
      expect(expected).toHaveLength(16);
      expect(palettes[base].ansi).toEqual(expected);
    }
  });
});

describe("Tauri theme tokens", () => {
  it("keeps every base aligned with the terminal palette contract", () => {
    for (const palette of Object.values(palettes) as Palette[]) {
      expect(palette.ansi).toHaveLength(16);
      expect(palette.termBg).toMatch(/^#[0-9a-f]{6}$/);
      expect(palette.termFg).toMatch(/^#[0-9a-f]{6}$/);
    }
  });

  /**
   * The window-control inset is a platform fact, and `applyTheme` writes every
   * variable here as an *inline style on the root element*. An inline default
   * outranks any stylesheet rule, including the `html[data-platform="macos"]`
   * one that says 68px — so emitting this token from the pipeline collapses the
   * title bar's left padding to 8px and puts the sidebar toggle on top of the
   * close button.
   *
   * That is not hypothetical: it is what happens, and this test is here because
   * it happened. The inset belongs to `styles/base.css` alone.
   */
  it("never emits the traffic-light inset", () => {
    for (const base of Object.keys(palettes) as ThemeBaseId[]) {
      const variables = toCssVariables(palettes[base]);
      expect(Object.keys(variables), base).not.toContain("--forge-traffic-light-w");
      expect(Object.keys(variables), base).not.toContain("--forge-traffic-light");
    }
  });

  it("emits CSS variables for palette and geometry tokens", () => {
    const variables = toCssVariables(palettes["gruvbox-hard"]);
    expect(variables["--forge-bg"]).toBe("#1f1f1f");
    expect(variables["--forge-term-bg"]).toBe("#1f1f1f");
    expect(variables["--forge-ansi-15"]).toBe("#c7c7c7");
    expect(variables["--forge-control-md"]).toMatch(/^\d+px$/);
    expect(variables["--forge-hover"]).toBe("#292929");
    expect(variables["--forge-selected"]).toBe("#373737");
  });
});
