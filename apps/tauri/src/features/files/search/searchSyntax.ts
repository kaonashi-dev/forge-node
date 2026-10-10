import { hitSegments } from "./fileContentSearch";
import type { SyntaxToken } from "../../../shared/syntax/highlight";

export { highlightExcerpt, MAX_SYNTAX_EXCERPT_LENGTH } from "../../../shared/syntax/highlight";
export type { SyntaxToken } from "../../../shared/syntax/highlight";

export type SyntaxHit = { hit: boolean; tokens: SyntaxToken[] };

/** Match the whole line before splitting tokens, so punctuation cannot break a hit. */
export function syntaxHits(tokens: readonly SyntaxToken[], needle: string): SyntaxHit[] {
  const hits = hitSegments(tokens.map((token) => token.text).join(""), needle);
  let index = 0;
  let offset = 0;
  return hits.map(({ text, hit }) => {
    const pieces: SyntaxToken[] = [];
    let remaining = text.length;
    while (remaining > 0 && index < tokens.length) {
      const token = tokens[index];
      const length = Math.min(remaining, token.text.length - offset);
      if (length > 0)
        pieces.push({ text: token.text.slice(offset, offset + length), scope: token.scope });
      remaining -= length;
      offset += length;
      if (offset === token.text.length) {
        index += 1;
        offset = 0;
      }
    }
    return { hit, tokens: pieces };
  });
}
