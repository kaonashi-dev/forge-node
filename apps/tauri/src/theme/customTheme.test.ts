import { describe, expect, it } from "vitest";
import { MAX_THEME_BYTES, parseCustomTheme } from "./customTheme";
import { palettes } from "./tokens";
const fixture = () => ({ name: "My theme", mode: "dark", palette: { ...palettes.neutral } });
describe("custom theme import", () => {
  it("round trips an exported palette", () => {
    expect(parseCustomTheme(JSON.stringify(fixture()))).toEqual(fixture());
  });
  it("accepts light mode and trims names", () => {
    expect(
      parseCustomTheme(JSON.stringify({ ...fixture(), name: " Day ", mode: "light" })).name,
    ).toBe("Day");
  });
  it.each(["{", "null", "[]", "{}"])("rejects malformed documents: %s", (source) => {
    expect(() => parseCustomTheme(source)).toThrow();
  });
  it("rejects missing colors and CSS injection", () => {
    for (const bg of [undefined, "red", "#fff", "url(https://example.com)", 123])
      expect(() =>
        parseCustomTheme(JSON.stringify({ ...fixture(), palette: { ...fixture().palette, bg } })),
      ).toThrow("palette.bg");
  });
  it("requires sixteen validated ANSI colors", () => {
    for (const ansi of [[], Array(17).fill("#ffffff"), Array(16).fill("bad")])
      expect(() =>
        parseCustomTheme(JSON.stringify({ ...fixture(), palette: { ...fixture().palette, ansi } })),
      ).toThrow("palette.ansi");
  });
  it("rejects oversized input before parsing", () => {
    expect(() => parseCustomTheme(" ".repeat(MAX_THEME_BYTES + 1))).toThrow("16 KB");
    expect(() => parseCustomTheme("é".repeat(MAX_THEME_BYTES))).toThrow("16 KB");
  });
  it("rejects unsupported modes and empty names", () => {
    expect(() => parseCustomTheme(JSON.stringify({ ...fixture(), mode: "system" }))).toThrow(
      "Mode",
    );
    expect(() => parseCustomTheme(JSON.stringify({ ...fixture(), name: " " }))).toThrow("name");
  });

  it("validates saved contrast and the unadjusted palette", () => {
    for (const contrast of [-1, 101, 60.5, "60", null]) {
      expect(() =>
        parseCustomTheme(
          JSON.stringify({
            ...fixture(),
            adjustments: { palette: fixture().palette, contrast },
          }),
        ),
      ).toThrow("contrast");
    }
    expect(() =>
      parseCustomTheme(
        JSON.stringify({
          ...fixture(),
          adjustments: { palette: {}, contrast: 60 },
        }),
      ),
    ).toThrow("palette.bg");
  });
});

describe("custom theme finish", () => {
  const theme = (palette: unknown) => JSON.stringify({ name: "Ink", mode: "dark", palette });

  it("carries Calm's finish through an export and back", () => {
    const parsed = parseCustomTheme(theme(palettes["calm-dark"]));
    expect(parsed.palette.finish).toEqual(palettes["calm-dark"].finish);
  });

  it("leaves a palette without one to derive its tab and selection", () => {
    expect(parseCustomTheme(theme(palettes["forge-dark"])).palette.finish).toBeUndefined();
  });

  it("keeps hover optional and rejects anything that is not a colour", () => {
    const base = palettes["calm-dark"];
    const noHover = parseCustomTheme(
      theme({ ...base, finish: { tab: "#1c1c1f", rule: "#1f1f22" } }),
    );
    expect(noHover.palette.finish).toEqual({ tab: "#1c1c1f", rule: "#1f1f22" });
    for (const finish of [
      "#1c1c1f",
      { tab: "#1c1c1f" },
      { tab: "url(x)", rule: "#1f1f22" },
      { tab: "#1c1c1f", rule: "#1f1f22", hover: "red" },
    ]) {
      expect(() => parseCustomTheme(theme({ ...base, finish }))).toThrow(/finish/);
    }
  });
});
