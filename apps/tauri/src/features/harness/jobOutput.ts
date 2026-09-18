/**
 * How a batch of streamed job output folds into the tail already held.
 *
 * Two shapes, and the difference between them is the whole point of the
 * module. The common case — the daemon numbering lines in order — is an
 * append, reported as absolute indexes the caller writes straight into the
 * store so the array it already has keeps its identity. The rare cases — a
 * gap, a replay, a batch that lands early, or the tail finally coming due for
 * a trim — are a replace, and they are priced as one.
 *
 * A stream at 1 000 lines/s is ~125 batches a second (`performance.md` prices
 * the delta path at ≤125/s). Copying a 2 000-entry tail per batch is a quarter
 * of a million element copies a second for output nobody is reading closely,
 * and writing at absolute indexes into a fresh array leaves holes that put V8
 * into dictionary mode on top of it.
 */
export type JobOutput = {
  /** Absolute line number of `lines[0]`. */
  fromLine: number;
  /** `undefined` where a gap in the numbering left a hole. */
  lines: Array<string | undefined>;
};

export type OutputPlan =
  /** The batch was empty; the store is not touched at all. */
  | { kind: "none" }
  /** Write `lines[i]` at `lines` index `at + i`. The array grows in place. */
  | { kind: "append"; at: number; lines: string[] }
  /** Swap the whole record: a gap, an overwrite, or a trim came due. */
  | { kind: "replace"; next: JobOutput };

export const EMPTY_OUTPUT: JobOutput = { fromLine: 0, lines: [] };

/**
 * How far past `budget` the tail is allowed to run before it is trimmed.
 *
 * Trimming is a slice, and a slice per batch is the copy this module exists to
 * avoid. Letting the tail overshoot means one slice per `slack` lines instead:
 * at 1 000 lines/s and the default slack that is roughly two copies a second,
 * not a hundred and twenty-five.
 */
export const OUTPUT_SLACK = 512;

/**
 * Decide how `lines`, numbered from `fromLine`, joins `current`.
 *
 * `overwrite` says what a line already held is worth. A live stream re-sending
 * a range means the later copy is the true one; a seed read from the log file
 * is filling gaps around output that arrived live, and must not clobber it.
 */
export function foldOutput(
  current: JobOutput,
  fromLine: number,
  lines: string[],
  budget: number,
  overwrite: boolean,
  slack: number = OUTPUT_SLACK,
): OutputPlan {
  if (lines.length === 0) return { kind: "none" };

  const end = current.fromLine + current.lines.length;
  const fits = current.lines.length + lines.length <= budget + slack;
  // The first batch of a job takes the replace path even though nothing is
  // being merged: it is what sets `fromLine`, and it costs one array.
  if (current.lines.length > 0 && fromLine === end && fits) {
    return { kind: "append", at: current.lines.length, lines };
  }
  return { kind: "replace", next: merge(current, fromLine, lines, budget, overwrite) };
}

function merge(
  current: JobOutput,
  fromLine: number,
  lines: string[],
  budget: number,
  overwrite: boolean,
): JobOutput {
  const currentEnd = current.fromLine + current.lines.length;
  const end = Math.max(currentEnd, fromLine + lines.length);
  // A job attached at line 40 000 has no line below it to hold. Anchoring the
  // window at zero would allocate forty thousand holes for output that is in
  // the log file and was never on the wire — the sparse array `performance.md`
  // warns about, and the reason this is not simply `end - budget`.
  const low = current.lines.length > 0 ? Math.min(current.fromLine, fromLine) : fromLine;
  const base = Math.max(0, end - budget, low);
  const next: Array<string | undefined> = new Array(Math.max(0, end - base)).fill(undefined);

  for (let index = 0; index < current.lines.length; index += 1) {
    const absolute = current.fromLine + index;
    if (absolute >= base) next[absolute - base] = current.lines[index];
  }
  for (let index = 0; index < lines.length; index += 1) {
    const absolute = fromLine + index;
    if (absolute < base) continue;
    if (overwrite || next[absolute - base] === undefined) next[absolute - base] = lines[index];
  }
  return { fromLine: base, lines: next };
}

/** The whole tail as one record, for a seed that replaces what came before. */
export function seedOutput(lines: string[], budget: number): JobOutput {
  const fromLine = Math.max(0, lines.length - budget);
  return { fromLine, lines: fromLine === 0 ? [...lines] : lines.slice(fromLine) };
}
