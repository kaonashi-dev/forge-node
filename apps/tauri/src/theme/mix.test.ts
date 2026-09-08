import { describe, expect, it } from "vitest";
import { contrast, derivedTokens, hex, mix, parseHex, pct, rgba } from "./mix";
import { palettes } from "./tokens";

describe("theme mix", () => {
  it("returns the base at 0 and the overlay at 255", () => {
    const bg = parseHex("#1f1f1f");
    const text = parseHex("#c7c7c7");
    expect(mix(bg, text, 0)).toBe(bg);
    expect(mix(bg, text, 255)).toBe(text);
  });

  it("keeps a grey mix grey", () => {
    for (const amount of [0, 1, 64, 127, 128, 200, 254, 255]) {
      const mixed = mix(0x0a0a0a, 0xfafafa, amount);
      expect(mixed & 0xff000000).toBe(0);
      const red = (mixed >> 16) & 0xff;
      const green = (mixed >> 8) & 0xff;
      const blue = mixed & 0xff;
      expect(red).toBe(green);
      expect(green).toBe(blue);
    }
  });

  it("matches pct() rounding", () => {
    expect(pct(6)).toBe(15);
    expect(pct(7)).toBe(18);
    expect(pct(14)).toBe(36);
  });

  it("climbs hover then selected away from the canvas", () => {
    const bg = parseHex("#1f1f1f");
    const text = parseHex("#c7c7c7");
    const hover = mix(bg, text, pct(6));
    const selected = mix(bg, text, pct(14));
    expect(hover).toBeGreaterThan(bg);
    expect(selected).toBeGreaterThan(hover);
    expect(hex(hover)).toBe("#292929");
    expect(hex(selected)).toBe("#373737");
  });

  it("measures contrast symmetrically", () => {
    expect(contrast(0x000000, 0xffffff)).toBeCloseTo(21, 5);
    expect(contrast(0xffffff, 0x000000)).toBeCloseTo(21, 5);
  });

  it("measures contrast symmetrically", () => {
    expect(contrast(0x000000, 0xffffff)).toBeCloseTo(21, 5);
    expect(contrast(0xffffff, 0x000000)).toBeCloseTo(21, 5);
  });
});

/**
 * The derived tokens, pinned to the mix formulas.
 *
 * The literal palette is checked against an exported fixture
 * (`tokens.test.ts`); these are the values *computed* from it, and a fixture
 * cannot check them because Rust computes them at paint time from a global.
 * So the formulas are restated here — including which ground each one mixes
 * into, which is the part that was wrong before this test existed: both diff
 * washes were mixed into `bg` at 16% instead of `editor` at 14%.
 */
describe("derived tokens", () => {
  const base = palettes["gruvbox-hard"];
  const args = {
    bg: parseHex(base.bg),
    text: parseHex(base.text),
    sidebar: parseHex(base.sidebar),
    editor: parseHex(base.editor),
    accent: parseHex(base.accent),
    amber: parseHex(base.amber),
    needsYou: parseHex(base.needsYou),
    gitAdded: parseHex(base.gitAdded),
    gitDeleted: parseHex(base.gitDeleted),
  };
  const tokens = derivedTokens(args);

  it("derives the interaction ramp from bg towards text", () => {
    expect(tokens["--forge-border"]).toBe(hex(mix(args.bg, args.text, pct(7))));
    expect(tokens["--forge-border-hi"]).toBe(hex(mix(args.bg, args.text, pct(15))));
    expect(tokens["--forge-hover"]).toBe(hex(mix(args.bg, args.text, pct(6))));
    expect(tokens["--forge-selected"]).toBe(hex(mix(args.bg, args.text, pct(14))));
    expect(tokens["--forge-pressed"]).toBe(hex(mix(args.bg, args.text, pct(20))));
  });

  it("derives the rail's own ramp from the rail, not the canvas", () => {
    expect(tokens["--forge-sidebar-hi"]).toBe(hex(mix(args.sidebar, args.text, pct(6))));
    expect(tokens["--forge-sidebar-pressed"]).toBe(hex(mix(args.sidebar, args.text, pct(14))));
  });

  // `theme::diff_added_bg` / `diff_removed_bg`: the patch pane is an editor
  // surface, and a wash mixed into the wrong ground reads as a seam down the
  // middle of the file.
  it("mixes both diff washes into the editor at 14%", () => {
    expect(tokens["--forge-diff-added-bg"]).toBe(hex(mix(args.editor, args.gitAdded, pct(14))));
    expect(tokens["--forge-diff-removed-bg"]).toBe(hex(mix(args.editor, args.gitDeleted, pct(14))));
  });

  it("derives the attention washes at their own strengths", () => {
    expect(tokens["--forge-needs-you-tint"]).toBe(hex(mix(args.bg, args.needsYou, pct(14))));
    expect(tokens["--forge-activity-tint"]).toBe(hex(mix(args.bg, args.amber, pct(12))));
  });

  // The one derived token that stays translucent: an outline is painted over
  // whatever the control sits on, so a colour mixed against one ground would
  // be wrong on every other.
  it("keeps the focus ring translucent accent", () => {
    expect(tokens["--forge-focus-ring"]).toBe(rgba(args.accent, 0.35));
    expect(tokens["--forge-focus-ring"]).toBe("rgb(108 172 189 / 0.35)");
  });

  it("derives a full set for every base", () => {
    const names = Object.keys(tokens);
    for (const [id, palette] of Object.entries(palettes)) {
      const derived = derivedTokens({
        bg: parseHex(palette.bg),
        text: parseHex(palette.text),
        sidebar: parseHex(palette.sidebar),
        editor: parseHex(palette.editor),
        accent: parseHex(palette.accent),
        amber: parseHex(palette.amber),
        needsYou: parseHex(palette.needsYou),
        gitAdded: parseHex(palette.gitAdded),
        gitDeleted: parseHex(palette.gitDeleted),
      });
      expect(Object.keys(derived), id).toEqual(names);
      for (const name of names) expect(derived[name], `${id} ${name}`).toBeTruthy();
    }
  });
});
