import { describe, expect, it } from "vitest";
import { combineFileIndex } from "./fileIndexData";
import { buildPathIndex, resolvePath } from "../references/pathref";
import type { FileTree } from "../../../contracts/workbench";

const tree = (paths: string[], truncated = false): FileTree => ({
  workspace_id: "w",
  truncated,
  entries: paths.map((path) => ({ path, kind: "File", ignored: false })),
});

describe("global navigation index", () => {
  it("finds files beneath closed folders and incorporates discovered ignored files", () => {
    const global = tree(["closed/deep/a.ts"]);
    const local = tree([".agents/ignored.md"]);
    const merged = combineFileIndex(global, local)!;
    expect(merged.entries.map((entry) => entry.path)).toEqual([
      ".agents/ignored.md",
      "closed/deep/a.ts",
    ]);
    expect(merged.truncated).toBe(false);
    expect(resolvePath("a.ts", buildPathIndex(merged.entries))).toBe("closed/deep/a.ts");
  });

  it("does not treat a partial index as proof of absence or a known folder as a file", () => {
    const local = tree(["known.ts"]);
    local.entries.push({ path: "dir.ts", kind: "Directory", ignored: false });
    const merged = combineFileIndex(null, local)!;
    const index = buildPathIndex(merged.entries, !merged.truncated);
    expect(resolvePath("missing.ts", index)).toBe("missing.ts");
    expect(resolvePath("dir.ts", index)).toBeNull();
    expect(resolvePath("known.ts", index)).toBe("known.ts");
  });
  it("does not infer basename uniqueness or absence from a partial index", () => {
    const index = buildPathIndex(tree(["src/config.ts"]).entries, false);
    expect(resolvePath("config.ts", index)).toBe("config.ts");
    expect(resolvePath("tools/config.ts", index)).toBe("tools/config.ts");
  });

  it("bounds combined entries before insertion and keeps current local metadata", () => {
    const global = tree(Array.from({ length: 50_000 }, (_, i) => `file-${i}.ts`));
    const local = tree(["ignored/new.ts", "file-0.ts"]);
    local.entries[1].kind = "Directory";
    const merged = combineFileIndex(global, local)!;
    expect(merged.entries).toHaveLength(50_000);
    expect(merged.truncated).toBe(true);
    expect(merged.entries[1].kind).toBe("Directory");
  });

  it("never imports entries from another checkout", () => {
    const local = { ...tree(["foreign.ts"]), workspace_id: "other" };
    expect(combineFileIndex(tree(["here.ts"]), local)?.entries.map((entry) => entry.path)).toEqual([
      "here.ts",
    ]);
  });
});
