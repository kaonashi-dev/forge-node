// Palette file ranking: `SearchFiles { kind: Name }` on the daemon, not the
// WebView. A second owner of `filesStore.search` would blank find-in-files.
import { createSignal, untrack } from "solid-js";
import type { SearchResults } from "../../../contracts/workbench";
import { searchFiles } from "../commands";
import { filesStore, setFilesStore } from "../state";
import { loading, setLoading } from "../../../state/loading";
import { nameSearchBusy as busy } from "./nameSearchState";

const NAME_SEARCH_DEBOUNCE_MS = 50;

type Asked = { workspace: string; needle: string };

const [asked, setAsked] = createSignal<Asked | null>(null);
const [pendingSearch, setPendingSearch] = createSignal(false);

let pending: ReturnType<typeof setTimeout> | undefined;

export function scheduleNameSearch(workspace: string | null, raw: string): void {
  clearTimeout(pending);
  setPendingSearch(false);
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
      (loading.nameSearch ||
        (filesStore.nameSearch?.workspace_id === workspace &&
          filesStore.nameSearch.query === needle))
    );
  });
  if (settled) return;
  setPendingSearch(true);
  pending = setTimeout(() => runNameSearch(workspace, needle), NAME_SEARCH_DEBOUNCE_MS);
}

export function runNameSearch(workspace: string, needle: string): void {
  clearTimeout(pending);
  setPendingSearch(false);
  setFilesStore({ nameSearch: null, nameSearchError: null });
  setAsked({ workspace, needle });
  setLoading("nameSearch", true);
  void searchFiles(workspace, needle, "name", 200).catch((error: unknown) => {
    setLoading("nameSearch", false);
    setFilesStore("nameSearchError", error instanceof Error ? error.message : String(error));
  });
}

export function nameResults(): SearchResults | null {
  const current = asked();
  const results = filesStore.nameSearch;
  if (!current || results?.workspace_id !== current.workspace || results.query !== current.needle) {
    return null;
  }
  return results;
}

/**
 * True from a keystroke until the reply for the query it asked lands.
 *
 * Not `loading.nameSearch`: any reply clears that flag, including one for an
 * earlier needle that arrives while the latest is still in flight.
 */
export function nameSearchBusy(): boolean {
  return busy(
    pendingSearch(),
    asked(),
    nameResults() !== null,
    filesStore.nameSearchError !== null,
  );
}
