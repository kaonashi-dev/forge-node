// The content search the Files sidebar and the Search tab share: one query, one
// debounce, one `git grep`. Two owners of the same `filesStore.search` slot
// would each clear it before asking and blank the other's answer.
import { createSignal, untrack } from "solid-js";
import { CONTENT_SEARCH_DEBOUNCE_MS, shouldSearchContent } from "./fileContentSearch";
import type { SearchResults } from "../../../contracts/workbench";
import { searchFiles } from "../commands";
import { filesStore, setFilesStore } from "../state";
import { loading, setLoading } from "../../../state/loading";

type Asked = { workspace: string; needle: string };

const [query, setQuery] = createSignal("");
const [asked, setAsked] = createSignal<Asked | null>(null);

export const contentQuery = query;
export const setContentQuery = setQuery;

let pending: ReturnType<typeof setTimeout> | undefined;

/**
 * Ask for `raw` after the debounce, unless that exact question is already
 * answered or out.
 *
 * Untracked on purpose: an effect that calls this must depend on the query and
 * the workspace only. Clearing the slot before a request would otherwise
 * re-run it and schedule the same grep again.
 */
export function scheduleContentSearch(workspace: string | null, raw: string): void {
  clearTimeout(pending);
  const needle = raw.trim();
  if (!workspace || !shouldSearchContent(needle)) {
    setAsked(null);
    return;
  }
  const settled = untrack(() => {
    const current = asked();
    return (
      current?.workspace === workspace &&
      current.needle === needle &&
      (loading.search ||
        (filesStore.search?.workspace_id === workspace && filesStore.search.query === needle))
    );
  });
  if (settled) return;
  pending = setTimeout(() => runContentSearch(workspace, needle), CONTENT_SEARCH_DEBOUNCE_MS);
}

export function runContentSearch(workspace: string, needle: string): void {
  clearTimeout(pending);
  setFilesStore({ search: null, searchError: null });
  setAsked({ workspace, needle });
  setLoading("search", true);
  void searchFiles(workspace, needle, "content").catch((error: unknown) => {
    setLoading("search", false);
    setFilesStore("searchError", error instanceof Error ? error.message : String(error));
  });
}

/** The needle the results on screen answer, for highlighting them. */
export function askedNeedle(): string | null {
  return asked()?.needle ?? null;
}

/** The answer to the question last asked, or `null` while it is out. */
export function contentResults(): SearchResults | null {
  const current = asked();
  const results = filesStore.search;
  if (!current || results?.workspace_id !== current.workspace || results.query !== current.needle)
    return null;
  return results;
}
