import { afterEach, describe, expect, it } from "vitest";
import { applyShellSnapshot, emptySnapshot } from "./forgeStore";
import {
  CYCLED_VIEWS,
  SIDEBAR_VIEWS,
  cycleSidebarView,
  cycleTarget,
  restoreSidebar,
  setSidebarOpen,
  setSidebarView,
  showView,
  sidebarOpen,
  sidebarView,
  toggleView,
} from "./sidebarStore";

afterEach(() => {
  setSidebarView("Projects");
  setSidebarOpen(true);
});

describe("toggleView", () => {
  it("closes the bar on a second press of the view already up", () => {
    showView("Files");
    toggleView("Files");
    expect(sidebarOpen()).toBe(false);
    expect(sidebarView()).toBe("Files");
  });

  it("switches to another view and leaves the bar open", () => {
    showView("Files");
    toggleView("History");
    expect(sidebarOpen()).toBe(true);
    expect(sidebarView()).toBe("History");
  });
});

describe("showView", () => {
  it("never closes the bar", () => {
    setSidebarOpen(false);
    showView("Git");
    expect(sidebarOpen()).toBe(true);
    expect(sidebarView()).toBe("Git");
  });
});

describe("restoreSidebar", () => {
  it("falls back to Projects when the stored view is not one", async () => {
    applyShellSnapshot({ ...emptySnapshot(), app_state: { "ui.sidebar.view": "Inspector" } });
    await Promise.resolve();
    restoreSidebar();
    expect(sidebarView()).toBe("Projects");
  });

  it("reads the stored view and open flag", async () => {
    applyShellSnapshot({
      ...emptySnapshot(),
      app_state: { "ui.sidebar.view": "Git", "ui.sidebar.open": "false" },
    });
    await Promise.resolve();
    restoreSidebar();
    expect(sidebarView()).toBe("Git");
    expect(sidebarOpen()).toBe(false);
  });
});

it("keeps Projects, Files and History as the first three views", () => {
  expect(SIDEBAR_VIEWS.slice(0, 3)).toEqual(["Projects", "Files", "History"]);
});

describe("CYCLED_VIEWS", () => {
  it("is exactly the views without a number chord", () => {
    expect(CYCLED_VIEWS).toEqual(["History", "PR", "Features", "Lieutenant", "Git"]);
  });
});

describe("cycleTarget", () => {
  // Closed on a cycled view counts as off the cycle too: the press that opens
  // the bar is not the one that advances.
  it("starts at History from closed or a numbered view", () => {
    expect(cycleTarget("Projects", true)).toBe("History");
    expect(cycleTarget("Files", true)).toBe("History");
    expect(cycleTarget("Git", false)).toBe("History");
  });

  it("walks the cycle in strip order and wraps", () => {
    expect(cycleTarget("History", true)).toBe("PR");
    expect(cycleTarget("PR", true)).toBe("Features");
    expect(cycleTarget("Features", true)).toBe("Lieutenant");
    expect(cycleTarget("Lieutenant", true)).toBe("Git");
    expect(cycleTarget("Git", true)).toBe("History");
  });
});

describe("cycleSidebarView", () => {
  it("opens on the first cycled view and advances from there", () => {
    showView("Projects");
    cycleSidebarView();
    expect(sidebarView()).toBe("History");
    cycleSidebarView();
    expect(sidebarView()).toBe("PR");
  });

  it("starts over at History after MOD-1, MOD-2, even from deep in the cycle", () => {
    showView("Features");
    toggleView("Projects");
    toggleView("Files");
    cycleSidebarView();
    expect(sidebarView()).toBe("History");
  });
});
