// Palette file ranking: `SearchFiles { kind: Name }` on the daemon, not the
// WebView. A second owner of `workbenchStore.search` would blank find-in-files.
import { createSignal, untrack } from "solid-js";
import { setLoading, setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import { searchFiles } from "./api";
import type { SearchResults } from "./types";

const NAME_SEARCH_DEBOUNCE_MS = 50;

type Asked = { workspace: string; needle: string };

const [asked, setAsked] = createSignal<Asked | null>(null);

let pending: ReturnType<typeof setTimeout> | undefined;

export function scheduleNameSearch(workspace: string | null, raw: string): void {
  clearTimeout(pending);
  const needle = raw.trim();
  if (!workspace || needle.length === 0) {
    setAsked(null);
    return;
  }
  const settled = untrack(() => {
    const current = asked();
    return (
      current?.workspace === workspace &&
      current.needle === needle &&
      (workbenchStore.loading.nameSearch ||
        (workbenchStore.nameSearch?.workspace_id === workspace &&
          workbenchStore.nameSearch.query === needle))
    );
  });
  if (settled) return;
  pending = setTimeout(() => runNameSearch(workspace, needle), NAME_SEARCH_DEBOUNCE_MS);
}

export function runNameSearch(workspace: string, needle: string): void {
  clearTimeout(pending);
  setWorkbenchStore({ nameSearch: null, nameSearchError: null });
  setAsked({ workspace, needle });
  setLoading("nameSearch", true);
  void searchFiles(workspace, needle, "name", 200).catch((error: unknown) => {
    setLoading("nameSearch", false);
    setWorkbenchStore("nameSearchError", error instanceof Error ? error.message : String(error));
  });
}

export function nameResults(): SearchResults | null {
  const current = asked();
  const results = workbenchStore.nameSearch;
  if (!current || results?.workspace_id !== current.workspace || results.query !== current.needle) {
    return null;
  }
  return results;
}
