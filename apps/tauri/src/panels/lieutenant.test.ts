import { describe, expect, it } from "vitest";
import { answerLines, jobIsFinal } from "./lieutenant";

describe("Lieutenant stream", () => {
  it("keeps the answer and summarizes commands", () => {
    expect(
      answerLines([
        "thread started  abc",
        "assistant  Feature 9 is waiting at the gate.",
        "$ rg spec_ready harness",
        "  ⤷ ok (0)  harness/features.json",
        "assistant  → Read",
      ]),
    ).toEqual(["Feature 9 is waiting at the gate.", "(2 command(s) run while answering)"]);
  });

  it("recognizes terminal job states", () => {
    expect(jobIsFinal("Queued")).toBe(false);
    expect(jobIsFinal("Running")).toBe(false);
    expect(jobIsFinal("Succeeded")).toBe(true);
    expect(jobIsFinal("Failed")).toBe(true);
    expect(jobIsFinal("Cancelled")).toBe(true);
  });
});
