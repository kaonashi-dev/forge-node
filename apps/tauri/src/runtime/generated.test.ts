import { describe, expect, it } from "vitest";
import { FIXTURE_NAMES, response_snapshot_populated } from "./generated/fixtures";

describe("generated fixtures", () => {
  it("lists every JSON fixture", () => {
    expect(FIXTURE_NAMES.length).toBeGreaterThan(50);
  });

  it("includes a populated snapshot with usage meters", () => {
    const snapshot = response_snapshot_populated.Snapshot;
    expect(snapshot.usage.length).toBeGreaterThan(0);
    expect(snapshot.sessions.length).toBeGreaterThan(0);
  });
});
