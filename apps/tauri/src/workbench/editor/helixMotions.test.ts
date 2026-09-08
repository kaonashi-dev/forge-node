import { describe, expect, it } from "vitest";
import { classOf, findUnbalanced, wordBoundary } from "./helixMotions";

describe("classOf", () => {
  it("splits into word, space and punctuation", () => {
    expect(classOf("a")).toBe("word");
    expect(classOf("_")).toBe("word");
    expect(classOf("7")).toBe("word");
    expect(classOf("é")).toBe("word");
    expect(classOf(" ")).toBe("space");
    expect(classOf("(")).toBe("punct");
  });

  it("treats the end of the document as space, so a motion stops there", () => {
    expect(classOf("")).toBe("space");
  });
});

describe("wordBoundary", () => {
  const doc = "const answer = 42;";

  it("runs forward to the end of the word it is in", () => {
    expect(wordBoundary(doc, 0, 1)).toBe(5); // `const`
  });

  it("skips the gap first, so a motion from a space selects the next word", () => {
    expect(wordBoundary(doc, 5, 1)).toBe(12); // space, then `answer`
  });

  it("runs backward to the start of the word behind it", () => {
    expect(wordBoundary(doc, 12, -1)).toBe(6); // back over `answer`
  });

  it("takes a run of punctuation as one word", () => {
    // `= 42;` — from the `=` forward is the `=` alone, not `= 42`.
    expect(wordBoundary("a === b", 2, 1)).toBe(5);
  });

  it("stops at the ends of the document rather than running off them", () => {
    expect(wordBoundary(doc, doc.length, 1)).toBe(doc.length);
    expect(wordBoundary(doc, 0, -1)).toBe(0);
  });

  it("goes nowhere in a document of only spaces", () => {
    expect(wordBoundary("     ", 2, 1)).toBe(5);
    expect(wordBoundary("     ", 2, -1)).toBe(0);
  });
});

describe("findUnbalanced", () => {
  it("finds the pair around the caret", () => {
    const doc = "f(x)";
    expect(findUnbalanced(doc, 2, "(", ")", -1)).toBe(1);
    expect(findUnbalanced(doc, 2, "(", ")", 1)).toBe(3);
  });

  it("counts nesting, so the inner pair wins from inside it", () => {
    const doc = "f(g(x))";
    expect(findUnbalanced(doc, 4, "(", ")", -1)).toBe(3);
    expect(findUnbalanced(doc, 4, "(", ")", 1)).toBe(5);
  });

  it("skips a balanced pair on the way out", () => {
    const doc = "f(g(x) + y)";
    // From after `g(x)`, the enclosing pair is the outer one.
    expect(findUnbalanced(doc, 7, "(", ")", -1)).toBe(1);
    expect(findUnbalanced(doc, 7, "(", ")", 1)).toBe(10);
  });

  it("treats a quote as its own partner, because it has no nesting", () => {
    const doc = 'a "b c" d';
    expect(findUnbalanced(doc, 4, '"', '"', -1)).toBe(2);
    expect(findUnbalanced(doc, 4, '"', '"', 1)).toBe(6);
  });

  it("says nothing when there is no pair to find", () => {
    expect(findUnbalanced("no brackets", 3, "(", ")", -1)).toBeNull();
    expect(findUnbalanced("no brackets", 3, "(", ")", 1)).toBeNull();
  });
});
