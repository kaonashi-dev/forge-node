import { describe, expect, it } from "vitest";
import { actionForDiskRead } from "./conflict";

describe("actionForDiskRead", () => {
  it("applies a new snapshot onto a clean buffer", () => {
    expect(
      actionForDiskRead({
        seenRevision: undefined,
        revision: "a",
        dirty: false,
        disk: "hello",
        mine: "",
      }),
    ).toBe("apply");
  });

  it("applies a later snapshot onto a clean buffer", () => {
    expect(
      actionForDiskRead({
        seenRevision: "a",
        revision: "b",
        dirty: false,
        disk: "new",
        mine: "hello",
      }),
    ).toBe("apply");
  });

  it("does not treat a local edit as a disk change", () => {
    expect(
      actionForDiskRead({
        seenRevision: "a",
        revision: "a",
        dirty: true,
        disk: "hello",
        mine: "hello!",
      }),
    ).toBe("ignore");
  });

  it("conflicts when a new snapshot differs from a dirty buffer", () => {
    expect(
      actionForDiskRead({
        seenRevision: "a",
        revision: "b",
        dirty: true,
        disk: "agent",
        mine: "hello!",
      }),
    ).toBe("conflict");
  });

  it("matches when the new snapshot is the bytes we already have", () => {
    expect(
      actionForDiskRead({
        seenRevision: "a",
        revision: "b",
        dirty: true,
        disk: "hello!",
        mine: "hello!",
      }),
    ).toBe("matched");
  });

  it("ignores a re-read of the snapshot already on screen", () => {
    expect(
      actionForDiskRead({
        seenRevision: "a",
        revision: "a",
        dirty: false,
        disk: "hello",
        mine: "hello",
      }),
    ).toBe("ignore");
  });
});
