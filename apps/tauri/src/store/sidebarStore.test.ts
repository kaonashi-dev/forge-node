import { afterEach, describe, expect, it } from "vitest";
import { applyShellSnapshot, emptySnapshot } from "./forgeStore";
import {
  SIDEBAR_VIEWS,
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
