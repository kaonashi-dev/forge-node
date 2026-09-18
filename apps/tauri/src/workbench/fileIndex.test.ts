import { beforeEach, expect, it, vi } from "vitest";
import { setWorkbenchStore } from "../store/workbenchStore";
import { navigationFileIndex, navigationFilePaths } from "./fileIndex";

vi.mock("./directoryState", () => ({
  freshDirectoryTree: () => null,
  directories: { focus: () => undefined },
}));

beforeEach(() => {
  setWorkbenchStore({ workspace: "w", tree: null, treeStale: false });
});

it("refreshes the cached navigation entries when Solid retains the containing store object", () => {
  setWorkbenchStore("tree", {
    workspace_id: "w",
    entries: [{ path: "old.ts", kind: "File", ignored: false }],
    truncated: false,
  });
  const old = navigationFileIndex();
  expect(navigationFileIndex()).toBe(old);
  setWorkbenchStore("tree", {
    workspace_id: "w",
    entries: [{ path: "new.ts", kind: "File", ignored: false }],
    truncated: false,
  });
  expect(navigationFileIndex()?.entries[0].path).toBe("new.ts");
  expect(navigationFileIndex()).not.toBe(old);
  setWorkbenchStore("treeStale", true);
  expect(navigationFileIndex()?.truncated).toBe(true);
});

it("caches file paths for the palette without filtering the listing on each keystroke", () => {
  setWorkbenchStore("tree", {
    workspace_id: "w",
    entries: [
      { path: "src", kind: "Directory", ignored: false },
      { path: "src/a.ts", kind: "File", ignored: false },
    ],
    truncated: false,
  });
  expect(navigationFilePaths()).toEqual(["src/a.ts"]);
  expect(navigationFilePaths()).toBe(navigationFilePaths());
});
