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
});
