import { describe, expect, it } from "vitest";
import { retargetViews, type ParkedViews } from "./views";
import { withMoved } from "./recentFiles";

describe("confirmed path moves", () => {
  it("merges duplicate preview identities but preserves independent editor sessions", () => {
    const active = { kind: "preview", path: "a.md" } as const;
    const moved = retargetViews(
      {
        open: [
          active,
          { kind: "preview", path: "b.md" },
          { kind: "editor-terminal", session: "one", path: "a.md" },
          { kind: "editor-terminal", session: "two", path: "b.md" },
        ],
        active,
      },
      "a.md",
      "b.md",
    );
    expect(moved.open).toHaveLength(3);
    expect(moved.active).toEqual({ kind: "preview", path: "b.md" });
  });
  it("maps descendants in open and active views without replacing editor identities", () => {
    const views: ParkedViews = {
      open: [
        { kind: "editor-terminal", session: "editor-1", path: "src/a.ts" },
        { kind: "preview", path: "src/readme.md" },
        { kind: "preview", path: "src-other/readme.md" },
      ],
      active: { kind: "editor-terminal", session: "editor-1", path: "src/a.ts" },
    };
    const moved = retargetViews(views, "src", "lib");
    expect(moved.open).toEqual([
      { kind: "editor-terminal", session: "editor-1", path: "lib/a.ts" },
      { kind: "preview", path: "lib/readme.md" },
      views.open[2],
    ]);
    expect(moved.active).toEqual(moved.open[0]);
    expect(moved.open[2]).toBe(views.open[2]);
    expect(retargetViews(moved, "src", "lib")).toBe(moved);
  });

  it("maps recent files only in the affected checkout and deduplicates destinations", () => {
    const recent = { a: ["src/a.ts", "lib/a.ts", "src-extra/a.ts"], b: ["src/a.ts"] };
    expect(withMoved(recent, "a", "src", "lib")).toEqual({
      a: ["lib/a.ts", "src-extra/a.ts"],
      b: ["src/a.ts"],
    });
    expect(withMoved(recent, "missing", "src", "lib")).toBe(recent);
  });
});
