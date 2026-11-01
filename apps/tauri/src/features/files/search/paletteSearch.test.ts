import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { reconcile } from "solid-js/store";
import { failNameSearch, nameSearchBusy, runNameSearch } from "./paletteSearch";
import { filesStore, resetFilesAnswers } from "../state";
import { loading, setLoading } from "../../../state/loading";
import { focusWorkspace } from "../../../state/workspace";

describe("palette name search failures", () => {
  beforeEach(() => {
    vi.stubGlobal("window", {});
    mockIPC(() => undefined);
    focusWorkspace("w");
    resetFilesAnswers();
    setLoading(reconcile({}));
  });
  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
  });

  it("drops a failure for a query the palette has since replaced", () => {
    runNameSearch("w", "pa");
    runNameSearch("w", "pal");
    failNameSearch("w", "pa", "timed out");
    expect(filesStore.nameSearchError).toBeNull();
    expect(loading.nameSearch).toBe(true);
    expect(nameSearchBusy()).toBe(true);
  });

  it("settles the asked query on its own failure", () => {
    runNameSearch("w", "pal");
    failNameSearch("w", "pal", "timed out");
    expect(filesStore.nameSearchError).toBe("timed out");
    expect(loading.nameSearch).toBe(false);
    expect(nameSearchBusy()).toBe(false);
  });

  it("drops a failure for another checkout", () => {
    runNameSearch("w", "pal");
    failNameSearch("other", "pal", "timed out");
    expect(filesStore.nameSearchError).toBeNull();
  });
});
