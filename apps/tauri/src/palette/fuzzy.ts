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
  // Walk the string in place. Spreading into code-point arrays allocated two
  // new arrays per candidate. File paths are ranked in the daemon.
  const hay = candidate.toLowerCase();
  let score = 0;
  let needleIndex = 0;
  let previousMatch = -2;
  for (let index = 0; index < hay.length; index += 1) {
    if (needleIndex >= needle.length) break;
    if (hay[index] !== needle[needleIndex]) continue;
    score += 1;
    const prev = index === 0 ? "" : hay[index - 1];
    const startsWord =
      index === 0 ||
      prev === " " ||
      prev === "-" ||
      prev === "/" ||
      (isUpper(candidate, index) && !isUpper(candidate, index - 1));
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

function isUpper(text: string, index: number): boolean {
  if (index < 0 || index >= text.length) return false;
  const character = text[index];
  return character === character.toUpperCase() && character !== character.toLowerCase();
}

export function score(query: string, candidate: string): number | null {
  return scoreWith(needleOf(query), candidate);
}

/**
 * How much a hit inside the basename beats one that only the directory made.
 *
 * Large enough to outrank any path score, because these are answers to
 * different questions: `pal` meaning `CommandPalette.tsx` and `pal` meaning
 * "something under src/palette/" are not two degrees of the same match, and
 * interleaving them puts the file being reached for below eight it is not.
 */
const BASENAME_BONUS = 1_000;

/** An exact basename, which is as good as this gets. */
const EXACT_BONUS = 500;

/** Per directory between the checkout root and the file, for a path-only hit. */
const DEPTH_PENALTY = 2;

/**
 * Score a workspace path the way a "go to file" box should.
 *
 * The basename is tried first and, when it matches, wins outright. That is the
 * difference between this and scoring the whole path: eight files called
 * `SKILL.md` score identically on their path prefixes and fall back to
 * alphabetical order, so the list reads as a directory dump rather than as an
 * answer. A separator in the query means the directory *is* the question, so
 * that case skips straight to the path.
 *
 * Deliberately not folded into `scoreWith`: commands, sessions and branches
 * are labels rather than paths, and giving them a basename would mean deciding
 * that the half of "New Terminal" after a space is the important half.
 */
function slashCount(path: string): number {
  let n = 0;
  for (let i = 0; i < path.length; i += 1) {
    if (path.charCodeAt(i) === 47) n += 1;
  }
  return n;
}

export function scorePath(needle: string[], path: string): number | null {
  if (needle.length === 0) return 0;
  const depth = slashCount(path);

  if (!needle.includes("/")) {
    const base = path.slice(path.lastIndexOf("/") + 1);
    const onBase = scoreWith(needle, base);
    if (onBase !== null) {
      const exact = base.toLowerCase() === needle.join("") ? EXACT_BONUS : 0;
      return onBase + BASENAME_BONUS + exact - depth;
    }
  }

  const onPath = scoreWith(needle, path);
  // A shallow file that matched on its path is likelier to be the one meant
  // than a deep one that matched on the same letters spread over more folders.
  return onPath === null ? null : onPath - depth * DEPTH_PENALTY;
}

/** Filter and rank workspace paths, best first. Ties keep the input order. */
export function filterPaths<T extends { path: string }>(entries: T[], query: string): T[] {
  if (query.trim() === "") return entries;
  const needle = needleOf(query);
  const scored: { score: number; index: number; entry: T }[] = [];
  for (let index = 0; index < entries.length; index += 1) {
    const entry = entries[index];
    const value = scorePath(needle, entry.path);
    if (value !== null) scored.push({ score: value, index, entry });
  }
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((item) => item.entry);
}

/** Rank paths and keep at most `limit`, so the palette never materialises every row. */
export function rankPaths(paths: readonly string[], query: string, limit: number): string[] {
  if (limit <= 0) return [];
  if (query.trim() === "") return paths.slice(0, limit);
  const needle = needleOf(query);
  const scored: { score: number; index: number; path: string }[] = [];
  for (let index = 0; index < paths.length; index += 1) {
    const path = paths[index];
    const value = scorePath(needle, path);
    if (value !== null) scored.push({ score: value, index, path });
  }
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  if (scored.length > limit) scored.length = limit;
  return scored.map((item) => item.path);
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
