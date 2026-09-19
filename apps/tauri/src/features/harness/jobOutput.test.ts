import { describe, expect, it } from "vitest";
import { EMPTY_OUTPUT, foldOutput, seedOutput, type JobOutput } from "./jobOutput";

const BUDGET = 100;
const SLACK = 8;

function apply(current: JobOutput, from: number, lines: string[], overwrite = true): JobOutput {
  const plan = foldOutput(current, from, lines, BUDGET, overwrite, SLACK);
  if (plan.kind === "none") return current;
  if (plan.kind === "replace") return plan.next;
  const next = current.lines.slice();
  for (let index = 0; index < plan.lines.length; index += 1)
    next[plan.at + index] = plan.lines[index];
  return { fromLine: current.fromLine, lines: next };
}

describe("foldOutput", () => {
  it("takes the replace path for the first batch, so fromLine is set", () => {
    const plan = foldOutput(EMPTY_OUTPUT, 40, ["a", "b"], BUDGET, true, SLACK);
    expect(plan).toEqual({ kind: "replace", next: { fromLine: 40, lines: ["a", "b"] } });
  });

  it("keeps the absolute line number a late-opened job starts at", () => {
    // 40 lines scrolled past before anyone attached; the tail starts at 40 and
    // the store must not pretend they were blank.
    const state = apply(EMPTY_OUTPUT, 40, ["a", "b"]);
    expect(state.fromLine).toBe(40);
    expect(state.lines).toEqual(["a", "b"]);
  });

  it("appends in place while the tail is inside budget + slack", () => {
    const state = apply(EMPTY_OUTPUT, 0, ["a"]);
    const plan = foldOutput(state, 1, ["b", "c"], BUDGET, true, SLACK);
    expect(plan).toEqual({ kind: "append", at: 1, lines: ["b", "c"] });
  });

  it("does nothing for an empty batch", () => {
    expect(foldOutput(EMPTY_OUTPUT, 0, [], BUDGET, true, SLACK)).toEqual({ kind: "none" });
  });

  it("trims only once the tail overshoots budget by slack, not every batch", () => {
    let state = seedOutput(
      Array.from({ length: BUDGET }, (_, i) => `l${i}`),
      BUDGET,
    );
    // Inside the slack the array is still allowed to grow past the budget.
    for (let batch = 0; batch < SLACK; batch += 1) {
      const from = state.fromLine + state.lines.length;
      const plan = foldOutput(state, from, [`x${batch}`], BUDGET, true, SLACK);
      expect(plan.kind).toBe("append");
      state = apply(state, from, [`x${batch}`]);
    }
    expect(state.lines.length).toBe(BUDGET + SLACK);
    // The next line is the one that pays for all of them.
    const from = state.fromLine + state.lines.length;
    expect(foldOutput(state, from, ["over"], BUDGET, true, SLACK).kind).toBe("replace");
    state = apply(state, from, ["over"]);
    expect(state.lines.length).toBe(BUDGET);
    expect(state.lines.at(-1)).toBe("over");
    expect(state.fromLine).toBe(BUDGET + SLACK + 1 - BUDGET);
  });

  it("leaves a hole where the numbering skipped", () => {
    let state = apply(EMPTY_OUTPUT, 0, ["a"]);
    state = apply(state, 3, ["d"]);
    expect(state.lines).toEqual(["a", undefined, undefined, "d"]);
  });

  it("lets a live batch overwrite, and a seed only fill", () => {
    const state = apply(EMPTY_OUTPUT, 0, ["a", "b"]);
    expect(apply(state, 1, ["B"], true).lines).toEqual(["a", "B"]);
    expect(apply(state, 1, ["B"], false).lines).toEqual(["a", "b"]);
  });

  it("drops lines that fell out of the window entirely", () => {
    const state = seedOutput(
      Array.from({ length: BUDGET * 2 }, (_, i) => `l${i}`),
      BUDGET,
    );
    expect(state.fromLine).toBe(BUDGET);
    expect(state.lines.length).toBe(BUDGET);
    // A replay of line 3 is older than anything kept, so nothing changes.
    const next = apply(state, 3, ["ancient"]);
    expect(next.lines[0]).toBe(`l${BUDGET}`);
  });
});

describe("seedOutput", () => {
  it("keeps the tail and numbers it from where it starts", () => {
    const seeded = seedOutput(["a", "b", "c"], 2);
    expect(seeded).toEqual({ fromLine: 1, lines: ["b", "c"] });
  });
});
