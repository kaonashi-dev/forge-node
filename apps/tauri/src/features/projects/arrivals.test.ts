import { describe, expect, it } from "vitest";
import { ARRIVAL_MS, createArrivals } from "./arrivals";

describe("arrivals", () => {
  it("announces only ids that appear after the first snapshot, and only while fresh", () => {
    const arrivals = createArrivals();
    arrivals.note([], 0);
    arrivals.note(["a", "b"], 10);
    expect(arrivals.arrived("a", 10)).toBe(false);
    arrivals.note(["a", "b", "c"], 100);
    expect(arrivals.arrived("c", 200)).toBe(true);
    // A card that first mounts when its project is unfolded later stays still.
    expect(arrivals.arrived("c", 100 + ARRIVAL_MS)).toBe(false);
    arrivals.note(["a", "b", "c"], 5000);
    expect(arrivals.arrived("c", 5001)).toBe(false);
  });
});
