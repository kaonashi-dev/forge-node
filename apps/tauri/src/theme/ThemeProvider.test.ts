import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createComputed, createRoot } from "solid-js";
import { applyThemeBase, themeBase, themePreference, type ThemePreference } from "./ThemeProvider";
import { baseIsLight, palettes } from "./tokens";

beforeEach(() => {
  vi.stubGlobal("document", { documentElement: { dataset: {}, style: { setProperty: vi.fn() } } });
});
afterEach(() => {
  applyThemeBase("gruvbox-hard");
  vi.unstubAllGlobals();
});

describe("theme selection", () => {
  it("updates the chosen option at the same time as the applied base", () => {
    applyThemeBase("ocean");
    expect(themePreference()).toBe("ocean");
    expect(themeBase()).toBe("ocean");
  });
  it("keeps system selected while resolving the OS palette", () => {
    vi.stubGlobal("window", { matchMedia: () => ({ matches: true }) });
    applyThemeBase("system");
    expect(themePreference()).toBe("system");
    expect(themeBase()).toBe("gruvbox-light");
  });
  it("falls back with a matching active option for invalid saved preferences", () => {
    for (const value of ["unknown", "toString", "{bad json"]) {
      applyThemeBase(value as ThemePreference);
      expect(themeBase()).toBe("gruvbox-hard");
      expect(themePreference()).toBe("gruvbox-hard");
    }
  });
  it("restores custom colors and refreshes consumers when replacing a custom palette", () => {
    createRoot((dispose) => {
      const observed: string[] = [];
      createComputed(() => {
        observed.push(palettes[themeBase()].bg);
      });
      for (const base of ["neutral", "neutral-light"] as const) {
        const source = JSON.stringify({
          name: base,
          mode: base === "neutral" ? "dark" : "light",
          palette: palettes[base],
        }) as ThemePreference;
        applyThemeBase(source);
        expect(themePreference()).toBe(source);
        expect(themeBase()).toBe("custom");
        expect(palettes.custom.bg).toBe(palettes[base].bg);
        expect(baseIsLight("custom")).toBe(base === "neutral-light");
      }
      expect(observed.slice(-2)).toEqual([palettes.neutral.bg, palettes["neutral-light"].bg]);
      expect(Object.keys(palettes)).not.toContain("custom");
      dispose();
    });
  });
});
