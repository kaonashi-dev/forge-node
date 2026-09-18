import { describe, expect, it } from "vitest";
import { asSessionTranscript } from "./externalTranscript";

describe("asSessionTranscript", () => {
  /* The one honest difference between the two captures: a transcript file has
     turns, a terminal has lines. The dialog reads `lines`. */
  it("maps a run's turns onto the dialog's lines", () => {
    expect(
      asSessionTranscript({ session_id: "ses", text: "You: hi", turns: 3, truncated: true }),
    ).toEqual({ session_id: "ses", text: "You: hi", lines: 3, truncated: true });
  });
});
