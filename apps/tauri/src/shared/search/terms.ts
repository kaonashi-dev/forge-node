/**
 * A typed filter split into lowercase words; a row must contain each of them,
 * in any order, so `login fix` finds "Fix the login bug".
 */
export function termsOf(query: string): string[] {
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter((term) => term !== "");
}

/** `haystack` must already be lowercase: callers build it once per row, not per keystroke. */
export function hasAllTerms(haystack: string, terms: readonly string[]): boolean {
  return terms.every((term) => haystack.includes(term));
}
