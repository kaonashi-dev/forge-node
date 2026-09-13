import { describe, expect, it } from "vitest";
import { pathBasename, workspaceBranchMeta, workspaceFolderLabel } from "./workspaceLabel";

describe("workspaceLabel", () => {
  it("takes the last path segment", () => {
    expect(pathBasename("/Users/me/worktrees/abc/test")).toBe("test");
    expect(pathBasename("/r/main/")).toBe("main");
    expect(workspaceFolderLabel({ branch: "main", path: "/r/checkout-a" })).toBe("checkout-a");
  });

  it("falls back to folder for branch meta when branch is missing", () => {
    expect(workspaceBranchMeta({ branch: null, path: "/r/checkout-a" })).toBe("checkout-a");
    expect(workspaceBranchMeta({ branch: "main", path: "/r/main" })).toBe("main");
  });
});
