// A5, second half: the whole file's changes as a column beside the scrollbar.
//
// The gutter stripe answers "did this line change" for the lines on screen.
// The ruler answers "where else" for the ones that are not — which in a file
// longer than a viewport is most of them, and is the difference between
// knowing a file is dirty and knowing where.
//
// The geometry is pure and lives here because it is the part with a right and
// a wrong answer: a tick at the wrong height sends the reader to the wrong
// place, and it needs no DOM to check.

import type { GitMark, GitMarks } from "./createEditor";

/** One band on the ruler, in percentages of the file's height. */
export type RulerTick = {
  /** Distance from the top, 0–100. */
  top: number;
  /** How tall, in the same units. Never below [`MIN_HEIGHT`]. */
  height: number;
  mark: GitMark;
  /** The line to jump to when this band is clicked (1-based). */
  line: number;
};

/**
 * The smallest band that is still visible, as a percentage.
 *
 * One line of a 2 000-line file is 0.05% — a band rounded to nothing. The
 * floor is what keeps a one-line change from being a tick nobody can see or
 * hit; it costs the ruler a little precision in a long file, which is the
 * right trade for a column three pixels wide.
 */
const MIN_HEIGHT = 0.4;

/**
 * Group marked lines into the bands the ruler paints.
 *
 * Consecutive lines carrying the same mark become one band: a fifty-line
 * insertion is one stripe, not fifty stacked divs, and a run that reads as a
 * single edit should look like one. A run of mixed marks breaks — an addition
 * inside a modification is two different things to the reader.
 */
export function rulerTicks(marks: GitMarks, totalLines: number): RulerTick[] {
  if (marks.size === 0 || totalLines <= 0) return [];

  // A `Map` from a patch is in hunk order; bands have to be built in line
  // order or a run is split wherever the patch happened to jump.
  const lines = [...marks.keys()]
    .filter((line) => line >= 1 && line <= totalLines)
    .sort((a, b) => a - b);

  const ticks: RulerTick[] = [];
  let start = 0;
  while (start < lines.length) {
    const mark = marks.get(lines[start])!;
    let end = start;
    while (
      end + 1 < lines.length &&
      lines[end + 1] === lines[end] + 1 &&
      marks.get(lines[end + 1]) === mark
    ) {
      end += 1;
    }
    const first = lines[start];
    const span = lines[end] - first + 1;
    const top = ((first - 1) / totalLines) * 100;
    ticks.push({
      top,
      // Clamped so the floor cannot push a band past the bottom edge.
      height: Math.min(Math.max((span / totalLines) * 100, MIN_HEIGHT), 100 - top),
      mark,
      line: first,
    });
    start = end + 1;
  }
  return ticks;
}

/**
 * The line a click at `fraction` down the ruler is asking for.
 *
 * The nearest band rather than the raw position: the bands are floored to a
 * visible height, so in a long file the pixel under the pointer is only
 * approximately the line it draws — and a click on a tick that scrolled
 * somewhere with nothing in it would read as broken.
 */
export function lineAt(
  ticks: readonly RulerTick[],
  fraction: number,
  totalLines: number,
): number | null {
  if (ticks.length === 0) return null;
  const wanted = Math.min(Math.max(fraction, 0), 1) * totalLines;
  let best = ticks[0];
  let distance = Number.POSITIVE_INFINITY;
  for (const tick of ticks) {
    const to = Math.abs(tick.line - wanted);
    if (to < distance) {
      distance = to;
      best = tick;
    }
  }
  return best.line;
}
