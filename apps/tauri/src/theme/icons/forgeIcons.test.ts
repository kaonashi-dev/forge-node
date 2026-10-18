import { describe, expect, it } from "vitest";
import { brandForProvider } from "./BrandIcon";
import { ICONS, type ForgeIconName } from "./forgeIcons";

const NAMES: ForgeIconName[] = [
  "git-branch",
  "git-pull-request",
  "arrow-left",
  "settings",
  "appearance",
  "stats",
  "check",
  "loader",
  "bell",
  "filter",
  "refresh",
  "copy",
  "external-link",
  "edit",
  "trash",
  "agent",
  "square-terminal",
  "panel-left-open",
  "panel-left-close",
  "chevron-left",
  "chevron-right",
  "chevron-down",
  "chevron-up",
  "close",
  "plus",
  "minus",
  "folder-open",
  "folder",
  "file",
  "file-code",
  "search",
  "message-square-plus",
  "columns-2",
  "list-checks",
  "history",
  "layers",
  "maximize",
  "more-horizontal",
  "image",
  "image-off",
  "sparkles",
  "keyboard",
  "folder-symlink",
  "worktree",
  "commit",
  "fetch",
  "push",
  "patch",
  "merged",
  "conflict",
  "spawn-child",
  "idle",
  "exited",
  "failed",
  "command-palette",
];

describe("forgeIcons", () => {
  it("backs every UI name with a component", () => {
    for (const name of NAMES) {
      expect(typeof ICONS[name], name).toBe("function");
    }
    expect(Object.keys(ICONS).sort()).toEqual([...NAMES].sort());
  });

  /**
   * Brands and states are deliberately absent from the stroke registry: a
   * provider logo is a `BrandIcon`, a session state is a geometric
   * `StateMarker`. A brand name leaking back in here would put a logotype on
   * the stroke path and lose the mask-tint the brand set depends on.
   */
  it("keeps brand and state marks out of the stroke registry", () => {
    for (const name of [
      "claude",
      "codex",
      "opencode",
      "cursor",
      "grok",
      "editor-zed",
      "state-running",
    ]) {
      expect(Object.keys(ICONS)).not.toContain(name);
    }
  });

  it("maps a provider to its brand, or to none when it has no logo", () => {
    expect(brandForProvider("claude")).toBe("claude");
    expect(brandForProvider("codex")).toBe("codex");
    expect(brandForProvider("opencode")).toBe("opencode");
    expect(brandForProvider("cursor")).toBe("cursor");
    expect(brandForProvider("grok")).toBe("grok");
    expect(brandForProvider("custom")).toBeNull();
  });
});
