import { describe, expect, it } from "vitest";
import { hasAllTerms, termsOf } from "./terms";

describe("terms", () => {
  it("matches every word in any order, ignoring case", () => {
    const terms = termsOf("  Login   FIX ");
    expect(terms).toEqual(["login", "fix"]);
    expect(hasAllTerms("fix the login bug", terms)).toBe(true);
    expect(hasAllTerms("fix the logout bug", terms)).toBe(false);
  });

  it("keeps every row for a blank query", () => {
    expect(hasAllTerms("anything", termsOf("   "))).toBe(true);
  });
});
