import { describe, expect, it } from "vitest";
import { parentPath, retargetPath, validatePath, validateRename } from "./paths";

describe("workspace path validation", () => {
  it.each([
    "",
    "/tmp/a",
    "C:/a",
    ".",
    "..",
    "a/../b",
    "a//b",
    "a/",
    ".git/config",
    "a/.git",
    "a\nb",
    "a\0b",
  ])("rejects %j", (path) => {
    expect(validatePath(path)).not.toBeNull();
  });
  it.each([".agents", "a b/c'd", "résumé/世界.md", "a\\b"])("permits Unix path %j", (path) => {
    expect(validatePath(path)).toBeNull();
  });
  it("measures path limits as UTF-8 bytes", () => {
    expect(validatePath("é".repeat(2048))).toBeNull();
    expect(validatePath("é".repeat(2049))).not.toBeNull();
  });
  it("rejects self/descendant moves while allowing case changes and sibling prefixes", () => {
    expect(validateRename("src", "src")).not.toBeNull();
    expect(validateRename("src", "src/nested")).not.toBeNull();
    expect(validateRename("src", "Src")).toBeNull();
    expect(validateRename("src", "src2")).toBeNull();
  });
  it("retargets on component boundaries", () => {
    expect(retargetPath("src/a", "src", "lib")).toBe("lib/a");
    expect(retargetPath("src", "src", "lib")).toBe("lib");
    expect(retargetPath("src2/a", "src", "lib")).toBe("src2/a");
    expect(parentPath("a")).toBe("");
    expect(parentPath("a/b")).toBe("a");
  });
});
