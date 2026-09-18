import type { SearchMatch } from "../../../contracts/workbench";

/** Below this, a content search would match too much of the checkout. */
export const MIN_CONTENT_QUERY = 2;

/** Wait out typing before spending a `git grep` on the workbench worker. */
export const CONTENT_SEARCH_DEBOUNCE_MS = 250;

export function shouldSearchContent(query: string): boolean {
  return query.trim().length >= MIN_CONTENT_QUERY;
}

/** One path's hits, in the order the daemon returned them. */
export type ContentHitGroup = {
  path: string;
  matches: SearchMatch[];
};

export function groupContentHits(matches: SearchMatch[]): ContentHitGroup[] {
  const groups: ContentHitGroup[] = [];
  const index = new Map<string, ContentHitGroup>();
  for (const match of matches) {
    let group = index.get(match.path);
    if (!group) {
      group = { path: match.path, matches: [] };
      index.set(match.path, group);
      groups.push(group);
    }
    group.matches.push(match);
  }
  return groups;
}

/** A run of one line, and whether that run is the needle. */
export type HitSegment = { text: string; hit: boolean };

/**
 * Every occurrence of `needle`, left to right and never overlapping — the same
 * case-sensitive fixed string `git grep -F` matched, so a highlight cannot
 * disagree with the hit count.
 */
export function hitSegments(text: string, needle: string): HitSegment[] {
  const segments: HitSegment[] = [];
  let from = 0;
  if (needle.length > 0) {
    for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, from)) {
      if (at > from) segments.push({ text: text.slice(from, at), hit: false });
      segments.push({ text: needle, hit: true });
      from = at + needle.length;
    }
  }
  if (from < text.length) segments.push({ text: text.slice(from), hit: false });
  return segments;
}

/** Characters a sidebar row keeps ahead of its first hit once it cuts the line. */
export const ROW_HIT_LEAD = 12;

/**
 * A sidebar row's segments, cut so the first hit is on screen.
 *
 * The row is a few dozen characters wide and ends in an ellipsis, so a hit
 * past that width would be counted and never shown.
 */
export function rowHitSegments(text: string, needle: string, lead = ROW_HIT_LEAD): HitSegment[] {
  const line = text.trim();
  const first = needle.length > 0 ? line.indexOf(needle) : -1;
  if (first <= lead) return hitSegments(line, needle);
  let cut = first - lead;
  // Never start inside a surrogate pair.
  const unit = line.charCodeAt(cut);
  if (unit >= 0xdc00 && unit <= 0xdfff) cut += 1;
  return [{ text: "…", hit: false }, ...hitSegments(line.slice(cut), needle)];
}

export type ExcerptLine = { line: number; text: string; hit: boolean };

/** One file in the project-search tab: its hits and the context around them. */
export type FileExcerpts = {
  path: string;
  hits: number;
  /** Runs of consecutive lines; a gap between runs is lines nobody asked for. */
  excerpts: ExcerptLine[][];
};

/**
 * Merge each hit's context into contiguous runs, per file, in daemon order.
 *
 * Two hits whose context touches share one run instead of printing the lines
 * between them twice.
 */
export function buildExcerpts(matches: SearchMatch[]): FileExcerpts[] {
  const files: { path: string; hits: number; lines: Map<number, ExcerptLine> }[] = [];
  const index = new Map<string, (typeof files)[number]>();
  for (const match of matches) {
    let file = index.get(match.path);
    if (!file) {
      file = { path: match.path, hits: 0, lines: new Map() };
      index.set(match.path, file);
      files.push(file);
    }
    file.hits += 1;
    const lines = file.lines;
    const keep = (line: number, text: string) => {
      if (line >= 1 && !lines.has(line)) lines.set(line, { line, text, hit: false });
    };
    match.before.forEach((text, i) => keep(match.line - match.before.length + i, text));
    lines.set(match.line, { line: match.line, text: match.text, hit: true });
    match.after.forEach((text, i) => keep(match.line + 1 + i, text));
  }
  return files.map(({ path, hits, lines }) => {
    const excerpts: ExcerptLine[][] = [];
    for (const entry of [...lines.values()].sort((a, b) => a.line - b.line)) {
      const run = excerpts[excerpts.length - 1];
      if (run && run[run.length - 1].line + 1 === entry.line) run.push(entry);
      else excerpts.push([entry]);
    }
    return { path, hits, excerpts };
  });
}
