import type { SearchMatch } from "../workbench/types";

/** Below this, a content search would match too much of the checkout. */
export const MIN_CONTENT_QUERY = 2;

/** Wait out typing before spending a `git grep` on the workbench worker. */
export const CONTENT_SEARCH_DEBOUNCE_MS = 250;

export function shouldSearchContent(query: string): boolean {
  return query.trim().length >= MIN_CONTENT_QUERY;
}

/** One path's hits, in the order the daemon returned them. */
export type ContentHitGroup = {
  path: string;
  matches: SearchMatch[];
};

export function groupContentHits(matches: SearchMatch[]): ContentHitGroup[] {
  const groups: ContentHitGroup[] = [];
  const index = new Map<string, ContentHitGroup>();
  for (const match of matches) {
    let group = index.get(match.path);
    if (!group) {
      group = { path: match.path, matches: [] };
      index.set(match.path, group);
      groups.push(group);
    }
    group.matches.push(match);
  }
  return groups;
}
