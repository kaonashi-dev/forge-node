import { describe, expect, it } from "vitest";
import { createArrivals } from "./arrivals";

describe("arrivals", () => {
  it("announces only ids that appear after the first snapshot, once each", () => {
    const arrivals = createArrivals();
    expect(arrivals.arrived("a")).toBe(false);
    arrivals.prime(["a", "b"]);
    arrivals.prime(["c"]);
    expect(arrivals.arrived("a")).toBe(false);
    expect(arrivals.arrived("c")).toBe(true);
    expect(arrivals.arrived("c")).toBe(false);
  });
});
