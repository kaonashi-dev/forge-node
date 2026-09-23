export type Segment = { text: string; hit: boolean };

/**
 * Split `label` into the runs a query matched and the runs it did not.
 *
 * A contiguous hit is preferred over the first subsequence: `work` in
 * "New worktree from branch…" should light the word, not the `w` of "New".
 * When the label alone cannot hold the whole query (a file row matched on its
 * directory, a session on its branch), nothing is lit rather than a fragment
 * that reads as the reason for the match.
 */
export function highlight(label: string, query: string): Segment[] {
  const needle = query.toLowerCase().replace(/\s+/g, "");
  if (needle === "") return [{ text: label, hit: false }];
  const hay = label.toLowerCase();
  // Lowercasing can change a string's length (`İ`), and then no index lines up.
  if (hay.length !== label.length) return [{ text: label, hit: false }];
  const hits = new Array<boolean>(label.length).fill(false);

  const at = hay.indexOf(needle);
  if (at >= 0) {
    hits.fill(true, at, at + needle.length);
  } else {
    let cursor = 0;
    for (let index = 0; index < hay.length && cursor < needle.length; index += 1) {
      if (hay[index] === needle[cursor]) {
        hits[index] = true;
        cursor += 1;
      }
    }
    if (cursor < needle.length) return [{ text: label, hit: false }];
  }

  const segments: Segment[] = [];
  for (let index = 0; index < label.length; index += 1) {
    const last = segments[segments.length - 1];
    if (last && last.hit === hits[index]) last.text += label[index];
    else segments.push({ text: label[index], hit: hits[index] });
  }
  return segments;
}
