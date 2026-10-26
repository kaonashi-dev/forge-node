/**
 * A palette query as every matcher sees it: lowercased, a leading `./` dropped
 * and all whitespace removed, so `./src/app shell` is `src/appshell`. The
 * daemon's Go-to-file ranking (`fs_service::name_search`) normalizes the same
 * way; a highlighter that did not would light nothing on a row it returned.
 */
export function foldQuery(query: string): string {
  const trimmed = query.trim().toLowerCase();
  const relative = trimmed.startsWith("./") ? trimmed.slice(2) : trimmed;
  return relative.replace(/\s+/g, "");
}
