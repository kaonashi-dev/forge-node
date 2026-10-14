/**
 * Whether `text.trim()` would be empty, without allocating the trimmed copy.
 *
 * Runs usually end in padding, so the copy is paid on most runs of a frame.
 */
export function isBlank(text: string): boolean {
  for (let index = 0; index < text.length; index += 1) {
    if (!isTrimmedSpace(text.charCodeAt(index))) return false;
  }
  return true;
}

/** ECMAScript's WhiteSpace and LineTerminator code units, which `trim` removes. */
function isTrimmedSpace(code: number): boolean {
  if (code === 0x20) return true;
  if (code < 0x80) return code >= 0x09 && code <= 0x0d;
  return (
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000 ||
    code === 0xfeff
  );
}
