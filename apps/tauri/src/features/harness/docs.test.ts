import { describe, expect, it } from "vitest";
import { GATE_DOCS, RUN_DOCS, docPath, missingDocNote } from "./docs";

describe("harness documents", () => {
  // The slug is the half of a spec path that cannot be derived from the id.
  it("puts the spec documents under the numbered slug directory", () => {
    expect(docPath("Requirements", 7, "grok-support")).toBe(
      "harness/specs/7-grok-support/requirements.md",
    );
    expect(docPath("Design", 7, "grok-support")).toBe("harness/specs/7-grok-support/design.md");
    expect(docPath("Tasks", 7, "grok-support")).toBe("harness/specs/7-grok-support/tasks.md");
  });

  it("puts everything the run writes under progress/, keyed by id alone", () => {
    expect(docPath("Gate", 7, "x")).toBe("harness/progress/gate_7.md");
    expect(docPath("Review", 7, "x")).toBe("harness/progress/review_7.md");
    expect(docPath("Impl", 7, "x")).toBe("harness/progress/impl_7.md");
    expect(docPath("Context", 7, "x")).toBe("harness/progress/context_7.md");
    expect(docPath("Current", 7, "x")).toBe("harness/progress/current_7.md");
  });

  // "There is no gate file" is a fact about the run, not a caption: the note
  // names the file that was looked for either way.
  it("names the file it looked for whether the read failed or found nothing", () => {
    expect(missingDocNote("harness/progress/gate_7.md", null)).toBe(
      "harness/progress/gate_7.md is missing or empty.",
    );
    expect(missingDocNote("harness/progress/gate_7.md", "permission denied")).toBe(
      "harness/progress/gate_7.md could not be read: permission denied",
    );
  });

  it("leads the gate with the gate file and the run with the verdict", () => {
    expect(GATE_DOCS[0].kind).toBe("Gate");
    expect(RUN_DOCS[0].kind).toBe("Review");
  });
});
