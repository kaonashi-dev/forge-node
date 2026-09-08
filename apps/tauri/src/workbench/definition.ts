// What to do with the hits a definition search came back with.
//
// Pure, and in its own module so a node test can read it: this is the half of
// "go to definition" that has a right and a wrong answer. The search is a
// heuristic (`fs-service::definition_rank`), so the rule here is that a single
// answer is jumped to and several are *offered* — never silently picked, which
// is how a guess starts reading as a fact.

import type { SearchMatch, SearchResults } from "./types";

/** Where the request came from, so the line it was made on is not an answer. */
export type Origin = { path: string; line: number };

export type Resolution =
  | { kind: "jump"; target: SearchMatch }
  | { kind: "choose"; targets: SearchMatch[]; truncated: boolean }
  | { kind: "none" };

/** How many candidates the editor offers before saying there are more. */
export const MAX_CANDIDATES = 20;

/**
 * Decide what a search for `symbol` means for the caret at `origin`.
 *
 * `results` is dropped whole when it answers a different question: the daemon
 * tags an answer with its query and nothing else, so a second lookup started
 * before the first landed is told apart by that and by nothing else.
 */
export function resolveDefinition(
  results: SearchResults | null,
  symbol: string,
  origin: Origin,
): Resolution {
  if (!results || results.query !== symbol) return { kind: "none" };
  // The declaration the caret is already on is not somewhere to go.
  const targets = results.matches.filter(
    (match) => !(match.path === origin.path && match.line === origin.line),
  );
  if (targets.length === 0) return { kind: "none" };
  if (targets.length === 1) return { kind: "jump", target: targets[0] };
  return {
    kind: "choose",
    targets: targets.slice(0, MAX_CANDIDATES),
    truncated: results.truncated || targets.length > MAX_CANDIDATES,
  };
}

/** One candidate as `path:line` — the shape the rest of the shell links in. */
export function candidateLabel(match: SearchMatch): string {
  return `${match.path}:${match.line}`;
}
