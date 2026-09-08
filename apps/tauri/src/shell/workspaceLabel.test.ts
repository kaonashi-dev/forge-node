import { describe, expect, it } from "vitest";
import {
  pathBasename,
  workspaceBranchMeta,
  workspaceFolderLabel,
  workspacePathSegment,
} from "./workspaceLabel";

describe("workspaceLabel", () => {
  it("takes the last path segment", () => {
    expect(pathBasename("/Users/me/worktrees/abc/test")).toBe("test");
    expect(pathBasename("/r/main/")).toBe("main");
  });

  it("shows folder only when it differs from the branch", () => {
    const same = { branch: "test", path: "/Users/me/worktrees/abc/test" };
    expect(workspaceFolderLabel(same)).toBe("test");
    expect(workspacePathSegment(same)).toBeNull();

    const different = { branch: "feature/foo", path: "/Users/me/worktrees/abc/my-folder" };
    expect(workspacePathSegment(different)).toBe("my-folder");
  });

  it("falls back to folder for branch meta when branch is missing", () => {
    expect(workspaceBranchMeta({ branch: null, path: "/r/checkout-a" })).toBe("checkout-a");
    expect(workspaceBranchMeta({ branch: "main", path: "/r/main" })).toBe("main");
  });
});
