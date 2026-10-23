import { describe, expect, it } from "vitest";
import { recallFinishes, recallTheme, rememberFinishes, rememberTheme } from "./lastTheme";

function memory() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
    removeItem: (key: string) => void values.delete(key),
  };
}

describe("last theme echo", () => {
  it("recalls what was remembered and forgets on undefined", () => {
    const storage = memory();
    expect(recallTheme(storage)).toBeUndefined();
    rememberTheme("gruvbox", storage);
    expect(recallTheme(storage)).toBe("gruvbox");
    rememberTheme(undefined, storage);
    expect(recallTheme(storage)).toBeUndefined();
  });

  it("survives storage that throws", () => {
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
      removeItem: () => {
        throw new Error("denied");
      },
    };
    expect(recallTheme(broken)).toBeUndefined();
    expect(() => rememberTheme("ocean", broken)).not.toThrow();
  });

  it("echoes the finishes and reads anything malformed as both off", () => {
    const storage = memory();
    expect(recallFinishes(storage)).toEqual({ glass: false, trim: false });
    rememberFinishes({ glass: true, trim: true }, storage);
    expect(recallFinishes(storage)).toEqual({ glass: true, trim: true });
    storage.setItem("forge.theme.finishes", "{not json");
    expect(recallFinishes(storage)).toEqual({ glass: false, trim: false });
    storage.setItem("forge.theme.finishes", '{"trim":"true"}');
    expect(recallFinishes(storage)).toEqual({ glass: false, trim: false });
  });
});
