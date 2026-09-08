// The identifier under a position, for "go to definition".
//
// Its own module, and not a CodeMirror `syntaxTree` walk, for two reasons: a
// node test can read it without CM6's DOM, and the answer must be the same for
// a file whose grammar chunk has not loaded yet — the daemon searches by word,
// so what is picked here has to be a word.

/** What a name may be made of. `$` for JS, `_` for everyone. */
function isSymbolChar(char: string): boolean {
  return /[A-Za-z0-9_$]/.test(char);
}

/**
 * The identifier `position` sits in or beside, or `null`.
 *
 * A caret between two characters belongs to the word on either side, which is
 * how a double-click and every editor's go-to-definition already behave: the
 * scan runs left from the position and right from it, so `open|File` and
 * `openFile|` are the same request.
 *
 * A run that starts with a digit is not a name (`0xff`, `42`) and is refused
 * here rather than sent to the daemon to be refused there.
 */
export function symbolAt(doc: string, position: number): string | null {
  if (position < 0 || position > doc.length) return null;
  let start = position;
  while (start > 0 && isSymbolChar(doc[start - 1])) start -= 1;
  let end = position;
  while (end < doc.length && isSymbolChar(doc[end])) end += 1;
  if (end === start) return null;
  const word = doc.slice(start, end);
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(word) ? word : null;
}
