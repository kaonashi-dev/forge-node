// An http(s) address named in terminal output.
//
// Pure. The same trailing-punctuation rule the markdown autolink uses, so a
// README and a terminal open the address that was written.

/** One address, as offsets into the text it was found in. */
export type UrlRef = {
  from: number;
  /** One past the last character, so `text.slice(from, to)` is the address. */
  to: number;
  url: string;
};

const URL = /https?:\/\/[^\s<>"'`)\]]+/gi;
const TRAILING = /[.,;:!?]+$/;

/** Every http(s) address in one line, in reading order. */
export function findUrlRefs(text: string): UrlRef[] {
  const refs: UrlRef[] = [];
  for (const match of text.matchAll(URL)) {
    const start = match.index;
    const raw = match[0];
    if (start === undefined || raw === undefined) continue;
    const body = raw.replace(TRAILING, "");
    // A scheme with no host (`http://`) is not an address.
    if (!/^https?:\/\/[^\s/]+/i.test(body)) continue;
    const url = body.replace(/^https?/i, (scheme) => scheme.toLowerCase());
    refs.push({ from: start, to: start + body.length, url });
  }
  return refs;
}
