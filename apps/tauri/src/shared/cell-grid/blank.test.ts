import { describe, expect, it } from "vitest";
import { isBlank } from "./blank";

describe("isBlank", () => {
  it("agrees with trim on every code unit", () => {
    for (let code = 0; code <= 0xffff; code += 1) {
      const text = String.fromCharCode(code);
      expect(isBlank(text)).toBe(text.trim() === "");
    }
  });

  it("agrees with trim on whole runs", () => {
    for (const text of ["", "    ", " \t　 ", "  x  ", "x", "  ", "😀 "]) {
      expect(isBlank(text)).toBe(text.trim() === "");
    }
  });
});
