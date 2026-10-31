import { describe, expect, it } from "vitest";
import { nameSearchBusy, shownAnswer } from "./nameSearchState";

const asked = { workspace: "w1", needle: "pal" };

describe("nameSearchBusy", () => {
  it("is busy while the debounce is pending", () => {
    expect(nameSearchBusy(true, null, false, false)).toBe(true);
  });

  it("stays busy until the asked query is answered", () => {
    expect(nameSearchBusy(false, asked, false, false)).toBe(true);
    expect(nameSearchBusy(false, asked, true, false)).toBe(false);
  });

  it("clears on a failure", () => {
    expect(nameSearchBusy(false, asked, false, true)).toBe(false);
  });

  it("is idle with nothing asked", () => {
    expect(nameSearchBusy(false, null, false, false)).toBe(false);
  });
});

describe("shownAnswer", () => {
  const first = { workspace_id: "w1", matches: ["a"] };
  const second = { workspace_id: "w1", matches: ["b"] };

  it("shows the settled reply", () => {
    expect(shownAnswer(null, "w1", "pal", first, false)).toEqual({
      workspace: "w1",
      results: first,
    });
  });

  it("keeps the previous answer while the next is in flight", () => {
    const previous = shownAnswer(null, "w1", "pa", first, false);
    expect(shownAnswer(previous, "w1", "pal", null, true)).toBe(previous);
  });

  it("returns the same object when the reply has not changed", () => {
    const previous = shownAnswer(null, "w1", "pal", first, true);
    expect(shownAnswer(previous, "w1", "pal", first, false)).toBe(previous);
  });

  it("replaces the held answer when the reply lands", () => {
    const previous = shownAnswer(null, "w1", "pa", first, false);
    expect(shownAnswer(previous, "w1", "pal", second, false)?.results).toBe(second);
  });

  it("drops the held answer on a workspace change", () => {
    const previous = shownAnswer(null, "w1", "pa", first, false);
    expect(shownAnswer(previous, "w2", "pal", null, true)).toBeNull();
  });

  it("drops the held answer when the query is cleared", () => {
    const previous = shownAnswer(null, "w1", "pa", first, false);
    expect(shownAnswer(previous, "w1", "  ", null, true)).toBeNull();
  });

  it("does not list another checkout's answer under the active one", () => {
    const stale = { workspace_id: "w1", matches: ["a"] };
    expect(shownAnswer(null, "w2", "pal", stale, false)).toBeNull();
    const held = shownAnswer(null, "w2", "pa", { workspace_id: "w2", matches: ["b"] }, false);
    expect(shownAnswer(held, "w2", "pal", stale, true)).toBe(held);
  });

  it("drops the held answer once the search is no longer busy", () => {
    const previous = shownAnswer(null, "w1", "pa", first, false);
    expect(shownAnswer(previous, "w1", "pal", null, false)).toBeNull();
  });
});
