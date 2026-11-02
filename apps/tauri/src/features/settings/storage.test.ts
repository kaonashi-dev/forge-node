import { describe, expect, it } from "vitest";
import { directoryOf, joinPath, legacyWorktreeCount } from "./storage";

const ws = (path: string, managed = true) => ({ path, managed_by_app: managed });

describe("legacyWorktreeCount", () => {
  it("counts only managed worktrees under the legacy worktrees root", () => {
    expect(
      legacyWorktreeCount(
        [
          ws("/old/worktrees/p/one"),
          ws("/old/worktrees/p/nested/two"),
          ws("/old/worktrees2/p/three"),
          ws("/old/worktrees/p/unmanaged", false),
          ws("/other/worktrees/p/four"),
        ],
        "/old",
      ),
    ).toBe(2);
  });

  it("is zero without a legacy directory", () => {
    expect(legacyWorktreeCount([ws("/old/worktrees/p/one")], null)).toBe(0);
  });

  it("tolerates a trailing separator on the legacy path", () => {
    expect(legacyWorktreeCount([ws("/old/worktrees/p/one")], "/old/")).toBe(1);
  });
});

describe("path helpers", () => {
  it("finds the directory of an absolute path", () => {
    expect(directoryOf("/home/dev/.forge-node/config.toml")).toBe("/home/dev/.forge-node");
    expect(directoryOf("/")).toBe("/");
  });

  it("joins without doubling the separator", () => {
    expect(joinPath("/home/dev/.forge-node/", "logs")).toBe("/home/dev/.forge-node/logs");
  });
});
