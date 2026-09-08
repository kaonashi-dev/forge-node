import { describe, expect, it } from "vitest";
import { symbolAt } from "./symbol";

describe("symbolAt", () => {
  const line = "  const openFile = read(path);";

  it("reads the word from inside it, from either edge", () => {
    const start = line.indexOf("openFile");
    expect(symbolAt(line, start)).toBe("openFile");
    expect(symbolAt(line, start + 4)).toBe("openFile");
    expect(symbolAt(line, start + "openFile".length)).toBe("openFile");
  });

  it("answers nothing between words", () => {
    expect(symbolAt(line, 0)).toBeNull();
    expect(symbolAt("a  b", 2)).toBeNull();
  });

  // A number is a word by the character rule and is not a name; sending it
  // would cost a `git grep` for something no language declares.
  it("refuses a run that is not a name", () => {
    expect(symbolAt("const n = 42;", 11)).toBeNull();
    expect(symbolAt("0xff", 2)).toBeNull();
  });

  it("keeps the characters a name is allowed", () => {
    expect(symbolAt("const $el = 1", 7)).toBe("$el");
    expect(symbolAt("let _private = 1", 6)).toBe("_private");
    expect(symbolAt("open_file()", 3)).toBe("open_file");
  });

  it("stays inside the document", () => {
    expect(symbolAt("abc", -1)).toBeNull();
    expect(symbolAt("abc", 9)).toBeNull();
    expect(symbolAt("", 0)).toBeNull();
  });
});
