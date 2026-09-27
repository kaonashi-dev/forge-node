import { describe, expect, it } from "vitest";
import { findUrlRefs } from "./urlref";

const urls = (text: string) => findUrlRefs(text).map((ref) => ref.url);

describe("findUrlRefs", () => {
  it("reads the address Vite prints, including the port and the trailing slash", () => {
    const text = "  ➜  Local:   http://localhost:5353/";
    const ref = findUrlRefs(text)[0];
    expect(ref?.url).toBe("http://localhost:5353/");
    expect(text.slice(ref?.from ?? 0, ref?.to ?? 0)).toBe("http://localhost:5353/");
  });

  it("drops the punctuation a sentence leaves on the address", () => {
    expect(urls("see http://localhost:5353/.")).toEqual(["http://localhost:5353/"]);
    expect(urls("(http://localhost:5353/)")).toEqual(["http://localhost:5353/"]);
  });

  it("keeps a query string, which a path token would have split on =", () => {
    expect(urls("http://localhost:5353/foo?bar=1&x=2")).toEqual([
      "http://localhost:5353/foo?bar=1&x=2",
    ]);
  });

  it("lowercases only the scheme, which is what the opener accepts", () => {
    expect(urls("HTTP://localhost:5353/")).toEqual(["http://localhost:5353/"]);
  });

  it("is not a file and not a scheme with no host", () => {
    expect(urls("src/main.rs:42")).toEqual([]);
    expect(urls("http://")).toEqual([]);
    expect(urls("use --host to expose")).toEqual([]);
  });
});
