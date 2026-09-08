// Subsequence scoring — subsequence scoring for the palette.
//
// Matches at a word boundary and runs of consecutive matches score higher,
// which is what makes "nt" find "New Terminal" ahead of "Agent Settings".

/**
 * The lowercased, space-stripped query characters a match is scored against.
 *
 * Hoisted out of the per-candidate loop by `filter`: the needle is invariant
 * across candidates, and in the file scope the candidate set is the whole
 * repository listing.
 */
export function needleOf(query: string): string[] {
  return [...query.toLowerCase()].filter((character) => character !== " ");
}

/**
 * Score `candidate` against an already-built needle, or `null` when the
 * query's characters do not all appear in order.
 */
export function scoreWith(needle: string[], candidate: string): number | null {
  if (needle.length === 0) return 0;
  const hay = [...candidate.toLowerCase()];
  const raw = [...candidate];

  let score = 0;
  let needleIndex = 0;
  let previousMatch = -2;
  for (let index = 0; index < hay.length; index += 1) {
    if (needleIndex >= needle.length) break;
    if (hay[index] !== needle[needleIndex]) continue;
    score += 1;
    const startsWord =
      index === 0 ||
      hay[index - 1] === " " ||
      hay[index - 1] === "-" ||
      hay[index - 1] === "/" ||
      (raw[index] === raw[index].toUpperCase() &&
        raw[index] !== raw[index].toLowerCase() &&
        raw[index - 1] !== raw[index - 1].toUpperCase());
    if (startsWord) score += 8;
    if (previousMatch === index - 1) score += 4;
    previousMatch = index;
    needleIndex += 1;
  }
  if (needleIndex < needle.length) return null;
  // A short label that used most of its characters is a better hit than a long
  // one that happened to contain the same letters.
  return score - Math.floor(hay.length / 8);
}

export function score(query: string, candidate: string): number | null {
  return scoreWith(needleOf(query), candidate);
}

/**
 * Filter by `query`, best match first.
 *
 * An empty query keeps the presentation order — the palette opens on a menu,
 * not on a ranking. Ties keep the original order too, so a query that matches
 * a whole group does not shuffle it on every keystroke.
 *
 * `search` wins over `label` when an entry carries one: a file row is labelled
 * with its basename but has to answer to `src/pal`, and a session row is
 * labelled with a terminal title but has to answer to the branch printed
 * beside it.
 */
export function filter<T extends { label: string; search?: string }>(
  entries: T[],
  query: string,
): T[] {
  if (query.trim() === "") return entries;
  const needle = needleOf(query);
  const scored: { score: number; index: number; entry: T }[] = [];
  entries.forEach((entry, index) => {
    const value = scoreWith(needle, entry.search ?? entry.label);
    if (value !== null) scored.push({ score: value, index, entry });
  });
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((item) => item.entry);
}
