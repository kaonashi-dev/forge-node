// What the find bar's counter says, as a pure function of a search cursor.
//
// Split from `searchPanel.ts` because that half builds DOM: this one has a
// right and a wrong answer, so it stays importable by a node test.

/**
 * How many matches are counted before the tally stops.
 *
 * A file in this editor is at most `fs-service::MAX_FILE_BYTES`, but a
 * one-character query in a 2 MiB file is hundreds of thousands of matches and
 * the count is recomputed on every keystroke. Past the cap the label reads
 * `5000+` rather than looking exhaustive.
 */
export const MATCH_CAP = 5_000;

export type MatchTally = {
  /** Matches found, never more than the cap. */
  total: number;
  /** 1-based ordinal of the match the selection sits on, when it sits on one. */
  current: number | null;
  /** The scan stopped at the cap, so `total` is a floor. */
  capped: boolean;
};

/** A tally, or the two states that have no number to show. */
export type MatchStatus = MatchTally | "empty" | "invalid";

export function tallyMatches(
  matches: Iterator<{ from: number; to: number }>,
  selection: { from: number; to: number },
  cap: number = MATCH_CAP,
): MatchTally {
  let total = 0;
  let current: number | null = null;
  for (let step = matches.next(); !step.done; step = matches.next()) {
    total += 1;
    // The selection lands exactly on a match only because `findNext` put it
    // there; a caret elsewhere in the file leaves the ordinal unknown, which
    // is the "17 matches" case rather than "1 of 17".
    if (current === null && step.value.from === selection.from && step.value.to === selection.to) {
      current = total;
    }
    if (total >= cap) return { total, current, capped: true };
  }
  return { total, current, capped: false };
}

export function matchLabel(status: MatchStatus): string {
  if (status === "empty") return "";
  if (status === "invalid") return "Bad pattern";
  if (status.total === 0) return "No results";
  const total = status.capped ? `${status.total}+` : `${status.total}`;
  if (status.current !== null) return `${status.current} of ${total}`;
  return status.total === 1 ? "1 match" : `${total} matches`;
}
