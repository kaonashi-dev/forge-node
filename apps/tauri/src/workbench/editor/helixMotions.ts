// The two pieces of A10's keymap that are arithmetic rather than dispatch.
//
// Their own module so a node test can read them without CodeMirror: a motion
// that lands one character off is a `d` that eats a bracket, and that is not
// something to find out by using it.

type CharClass = "word" | "space" | "punct";

/** Character classes, the way Helix splits a line into words. */
export function classOf(ch: string): CharClass {
  if (ch === "") return "space";
  if (/\s/.test(ch)) return "space";
  return /[\p{L}\p{N}_]/u.test(ch) ? "word" : "punct";
}

/**
 * Where `w` or `b` lands from `from`.
 *
 * Skips whitespace first — a motion that starts on a space belongs to the next
 * word, not to the gap — then runs to the end of the class it finds. Returns an
 * offset, and the caller makes it a *selection* from where it started, which is
 * the whole difference between Helix and vim: `d` and `c` and `y` never need to
 * know what a motion is, because the motion already left one behind.
 */
export function wordBoundary(doc: string, from: number, direction: 1 | -1): number {
  const limit = doc.length;
  let index = Math.max(0, Math.min(from, limit));
  /** The character the cursor would step over next, in this direction. */
  const ahead = () => (direction === 1 ? (doc[index] ?? "") : (doc[index - 1] ?? ""));
  const canStep = () => (direction === 1 ? index < limit : index > 0);

  while (canStep() && classOf(ahead()) === "space") index += direction;
  const kind = classOf(ahead());
  if (kind === "space") return index;
  while (canStep() && classOf(ahead()) === kind) index += direction;
  return index;
}

/**
 * The nearest unmatched `open`/`close` from `from`, in one direction.
 *
 * Scanned rather than parsed. A parse tree would be more correct for brackets
 * and no help at all for quotes, which are not nodes in most grammars — and
 * `mi"` has to work in the same keystroke as `mi(`. Nesting is counted, so
 * `mi(` from inside `f(g(x))` selects the inner pair.
 */
export function findUnbalanced(
  doc: string,
  from: number,
  open: string,
  close: string,
  direction: 1 | -1,
): number | null {
  // A quote is its own partner, so there is no nesting to count: the nearest
  // one in each direction is the pair.
  if (open === close) {
    for (let i = direction === 1 ? from : from - 1; i >= 0 && i < doc.length; i += direction) {
      if (doc[i] === open) return i;
    }
    return null;
  }

  // Going forward, an `open` deepens and a `close` at depth zero is the answer.
  const deepens = direction === 1 ? open : close;
  const closes = direction === 1 ? close : open;
  let depth = 0;
  for (let i = direction === 1 ? from : from - 1; i >= 0 && i < doc.length; i += direction) {
    const ch = doc[i];
    if (ch === deepens) depth += 1;
    else if (ch === closes) {
      if (depth === 0) return i;
      depth -= 1;
    }
  }
  return null;
}
