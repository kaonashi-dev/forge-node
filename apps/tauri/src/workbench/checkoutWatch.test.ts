import { describe, expect, it } from "vitest";
import { checkoutMoved, type CheckoutMark } from "./checkoutWatch";

const mark = (over: Partial<CheckoutMark> = {}): CheckoutMark => ({
  head: "abc123",
  dirty: false,
  ...over,
});

describe("checkoutMoved", () => {
  it("says nothing about an unmeasured status", () => {
    expect(checkoutMoved(mark(), mark({ head: null }))).toBe(false);
  });

  it("says nothing when no answer arrived at all", () => {
    expect(checkoutMoved(mark(), null)).toBe(false);
  });

  it("has nothing to throw away on the first look", () => {
    expect(checkoutMoved(null, mark())).toBe(false);
  });

  it("stays still on an unchanged checkout", () => {
    expect(checkoutMoved(mark({ dirty: true }), mark({ dirty: true }))).toBe(false);
  });

  // A `git pull` rewrites HEAD while leaving the branch name alone, so the
  // OID is the only thing that separates it from a no-event.
  it("moves on a changed HEAD", () => {
    expect(checkoutMoved(mark(), mark({ head: "def456" }))).toBe(true);
  });

  it("moves when dirtiness flips under the same HEAD", () => {
    expect(checkoutMoved(mark(), mark({ dirty: true }))).toBe(true);
  });
});
