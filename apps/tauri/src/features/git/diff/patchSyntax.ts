import { highlightExcerpt, type SyntaxToken } from "../../../shared/syntax/highlight";
import type { PatchRow } from "./patch";
import type { Segment } from "./patchDocument";

// UTF-16 units across both sides, checked before building excerpt/token arrays.
export const MAX_PATCH_SYNTAX_LENGTH = 262_144;
export const MAX_PATCH_SYNTAX_ROWS = 4096;

type PatchSyntax = {
  before: Map<PatchRow, SyntaxToken[]>;
  after: Map<PatchRow, SyntaxToken[]>;
};

export function highlightPatch(path: string, rows: readonly PatchRow[]): PatchSyntax {
  const syntax: PatchSyntax = { before: new Map(), after: new Map() };
  if (rows.length > MAX_PATCH_SYNTAX_ROWS) return syntax;
  let length = 0;
  for (const row of rows) {
    if (row.kind === "meta" || row.kind === "hunk") continue;
    length += (row.text.length + 1) * (row.kind === "context" ? 2 : 1);
    if (length > MAX_PATCH_SYNTAX_LENGTH) return syntax;
  }

  let before: PatchRow[] = [];
  let after: PatchRow[] = [];
  function flush(): void {
    // Each side has its own lexer state; omitted hunk context must reset it.
    for (const [side, destination] of [
      [before, syntax.before],
      [after, syntax.after],
    ] as const) {
      const tokens = highlightExcerpt(
        path,
        side.map((row) => row.text),
      );
      side.forEach((row, index) =>
        destination.set(row, tokens[index] ?? [{ text: row.text, scope: null }]),
      );
    }
    before = [];
    after = [];
  }

  for (const row of rows) {
    if (row.kind === "hunk") flush();
    else if (row.kind === "removed") before.push(row);
    else if (row.kind === "added") after.push(row);
    else if (row.kind === "context") {
      before.push(row);
      after.push(row);
    }
  }
  flush();
  return syntax;
}

export function cachedPatchSyntax(path: () => string, rows: () => PatchRow[]) {
  let previousPath: string | undefined;
  let previousRows: PatchRow[] | undefined;
  let syntax: PatchSyntax = { before: new Map(), after: new Map() };
  return (
    row: PatchRow,
    side: "before" | "after" = row.kind === "removed" ? "before" : "after",
  ): SyntaxToken[] => {
    const currentPath = path();
    const currentRows = rows();
    if (currentPath !== previousPath || currentRows !== previousRows) {
      syntax = highlightPatch(currentPath, currentRows);
      previousPath = currentPath;
      previousRows = currentRows;
    }
    return syntax[side].get(row) ?? [{ text: row.text, scope: null }];
  };
}

export type SyntaxSegment = { changed: boolean; tokens: SyntaxToken[] };

export function syntaxSegments(tokens: SyntaxToken[], mark?: Segment): SyntaxSegment[] {
  if (!mark || mark.from >= mark.to) return [{ changed: false, tokens }];
  const segments: SyntaxSegment[] = [];
  let offset = 0;
  for (const token of tokens) {
    let start = 0;
    while (start < token.text.length) {
      const position = offset + start;
      const changed = position >= mark.from && position < mark.to;
      const end = Math.min(
        token.text.length,
        position < mark.from ? mark.from - offset : changed ? mark.to - offset : token.text.length,
      );
      let segment = segments.at(-1);
      if (!segment || segment.changed !== changed) {
        segment = { changed, tokens: [] };
        segments.push(segment);
      }
      segment.tokens.push({ text: token.text.slice(start, end), scope: token.scope });
      start = end;
    }
    offset += token.text.length;
  }
  return segments;
}
