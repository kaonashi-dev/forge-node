import { beforeEach, describe, expect, it } from "vitest";
import { setActiveWorkspace } from "../state/workspace";
import {
  centerMode,
  clearFindInFiles,
  currentViews,
  findInFilesPending,
  openEditorTerminal,
  requestFindInFiles,
  retargetWorkspaceViews,
  revealInTree,
  showSession,
  treeReveal,
  viewsStore,
} from "./viewsStore";

let workspace = 0;
beforeEach(() => {
  clearFindInFiles();
  setActiveWorkspace(`close-test-${++workspace}`);
});

describe("Find in Files", () => {
  it("keeps a reveal scoped to its checkout across switching and a confirmed move", () => {
    const original = `close-test-${workspace}`;
    revealInTree("src/a.ts");
    setActiveWorkspace("another-checkout");
    expect(treeReveal()).toBeNull();
    retargetWorkspaceViews(original, "src", "lib");
    setActiveWorkspace(original);
    expect(treeReveal()).toBe("lib/a.ts");
  });
  it("retargets parked editors and pending reveals by segments", () => {
    openEditorTerminal("nested", "src/a.ts", "parked");
    retargetWorkspaceViews("parked", "src", "lib");
    expect(viewsStore.byWorkspace.parked.active).toEqual({
      kind: "editor-terminal",
      session: "nested",
      path: "lib/a.ts",
    });
    revealInTree("src-other/a.ts");
    retargetWorkspaceViews(`close-test-${workspace}`, "src", "lib");
    expect(treeReveal()).toBe("src-other/a.ts");
  });

  it("opens a Code search tab from a session and reuses it on repeated requests", () => {
    showSession();
    requestFindInFiles();
    expect(centerMode()).toBe("code");
    expect(currentViews().active).toEqual({ kind: "search" });
    expect(findInFilesPending()).toEqual({ query: null });
    clearFindInFiles();
    requestFindInFiles();
    expect(currentViews().open.filter((view) => view.kind === "search")).toHaveLength(1);
    expect(findInFilesPending()).toEqual({ query: null });
  });

  it("keeps a supplied query pending until the search tab consumes it", () => {
    requestFindInFiles("wompi");
    expect(findInFilesPending()).toEqual({ query: "wompi" });
    expect(currentViews().active).toEqual({ kind: "search" });
    clearFindInFiles();
    expect(findInFilesPending()).toBeNull();
  });

  it("does not leave a search request for an unrelated checkout when none is selected", () => {
    setActiveWorkspace(null);
    showSession();
    requestFindInFiles();
    expect(findInFilesPending()).toBeNull();
    expect(centerMode()).toBe("session");
  });
});
