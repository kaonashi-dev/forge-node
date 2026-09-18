import { describe, expect, it } from "bun:test";
import { APP, defaultBindings, type Binding } from "../../../src/actions/actions";
import { chordSignature, mergeBindings } from "../../../src/actions/keymap";
import { parseChord } from "../../../src/actions/keys";

const defaults: Binding[] = [
  { action: "open_file_palette", context: APP, chord: parseChord("cmd-p") },
  { action: "open_file_palette", context: APP, chord: parseChord("cmd-o") },
  { action: "open_settings", context: APP, chord: parseChord("cmd-,") },
];
const chords = (bindings: Binding[]) =>
  bindings
    .filter((binding) => binding.action === "open_file_palette")
    .map((binding) => chordSignature(binding.chord));
describe("global open-file alias", () => {
  it("retains the old chord alongside Cmd+O", () => {
    expect(chords(mergeBindings([], defaults))).toEqual(["cmd-p", "cmd-o"]);
  });
  it("respects a file palette rebind and an explicit unbind", () => {
    expect(
      chords(
        mergeBindings([{ action: "open_file_palette", context: APP, chord: "cmd-u" }], defaults),
      ),
    ).toEqual(["cmd-u"]);
    expect(
      chords(mergeBindings([{ action: "open_file_palette", context: APP, chord: null }], defaults)),
    ).toEqual([]);
  });
  it("yields Cmd+O to an explicit other action", () => {
    const merged = mergeBindings(
      [{ action: "open_settings", context: APP, chord: "cmd-o" }],
      defaults,
    );
    expect(chords(merged)).toEqual(["cmd-p"]);
    expect(merged.find((binding) => chordSignature(binding.chord) === "cmd-o")?.action).toBe(
      "open_settings",
    );
  });
  it("uses the final preference when a prior override claimed Cmd+O", () => {
    expect(
      chords(
        mergeBindings(
          [
            { action: "open_settings", context: APP, chord: "cmd-o" },
            { action: "open_settings", context: APP, chord: "cmd-u" },
          ],
          defaults,
        ),
      ),
    ).toEqual(["cmd-p", "cmd-o"]);
  });
  it("installs the alias only on macOS", () => {
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
    try {
      Object.defineProperty(globalThis, "navigator", {
        configurable: true,
        value: { platform: "MacIntel" },
      });
      expect(chords(defaultBindings())).toContain("cmd-o");
      Object.defineProperty(globalThis, "navigator", {
        configurable: true,
        value: { platform: "Linux x86_64" },
      });
      expect(chords(defaultBindings())).not.toContain("cmd-o");
    } finally {
      if (descriptor) Object.defineProperty(globalThis, "navigator", descriptor);
      else Reflect.deleteProperty(globalThis, "navigator");
    }
  });
});
