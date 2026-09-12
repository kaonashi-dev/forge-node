/**
 * Markdown → blocks, for a pull request body or a document in the checkout.
 *
 * A parser rather than a dependency: this is the subset those actually use and
 * the bundle has a budget. It answers with data and never with HTML —
 * `Markdown.tsx` renders the blocks through JSX, so raw markup is text and
 * there is no sanitiser here to get wrong. The few tags a README leans on are
 * read as what they mean (`<img>`, `<br>`) or dropped as layout; none of them
 * is ever passed through.
 */

export type MdSpan =
  | { kind: "text"; text: string; strong: boolean; em: boolean }
  | { kind: "code"; text: string }
  | { kind: "link"; text: string; href: string }
  | MdImage;

/**
 * An image as the document wrote it. `src` is unresolved: whether it can be
 * drawn, and from where, depends on what the document is (`previewImages.ts`).
 */
export type MdImage = {
  kind: "image";
  alt: string;
  src: string;
  /** Where a click goes, for an image wrapped in a link; only a safe href. */
  href: string | null;
  /** From `<img width>`, in pixels or a percentage. */
  width: string | null;
};

export type MdListItem = {
  spans: MdSpan[];
  depth: number;
  /** `null` for an ordinary bullet; the box's state for a GFM task. */
  checked: boolean | null;
};

export type MdBlock =
  | { kind: "heading"; level: number; spans: MdSpan[] }
  | { kind: "paragraph"; spans: MdSpan[] }
  | { kind: "list"; ordered: boolean; items: MdListItem[] }
  | { kind: "code"; lang: string | null; text: string }
  | { kind: "quote"; blocks: MdBlock[] }
  | { kind: "table"; head: MdSpan[][]; rows: MdSpan[][][] }
  | { kind: "rule" };

/** Indent deeper than this folds back: a description is prose, not a tree. */
const MAX_DEPTH = 2;

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^\s`]*)/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE = /^ {0,3}([-*_])[ \t]*(?:\1[ \t]*){2,}$/;
const QUOTE = /^ {0,3}> ?(.*)$/;
const ITEM = /^([ \t]*)(?:([-*+])|(\d{1,9})[.)])\s+(.*)$/;
const TASK = /^\[([ xX])\]\s+(.*)$/;
const WORD = /[\p{L}\p{N}_]/u;
const COMMENT_OPEN = /^ {0,3}<!--/;

/** `(src "title")` after an image's `![alt]`, the title ignored. */
const IMAGE_TARGET = String.raw`\(\s*(<[^>]*>|[^\s)]+)(?:\s+(?:"[^"]*"|'[^']*'))?\s*\)`;
const IMAGE = new RegExp(String.raw`^!\[([^\]]*)\]` + IMAGE_TARGET);
const LINKED_IMAGE = new RegExp(
  String.raw`^\[\s*!\[([^\]]*)\]` +
    IMAGE_TARGET +
    String.raw`\s*\]\(\s*([^\s)]+)(?:\s+"[^"]*")?\s*\)`,
);
const HTML_TAG = /^<(\/?)([a-z][a-z0-9]*)\b((?:[^<>"']|"[^"]*"|'[^']*')*)>/i;
const HTML_COMMENT = /^<!--[\s\S]*?-->/;

/**
 * Tags a README wraps images and badges in. They carry layout this renderer
 * does not do, so they are dropped rather than printed as text around the
 * picture they were centring.
 */
const LAYOUT_TAGS = new Set(["a", "p", "div", "center", "picture", "source", "span"]);

/** The blocks of one Markdown document, in reading order. */
export function parseMarkdown(source: string): MdBlock[] {
  return parseBlocks(source.replace(/\r\n?/g, "\n").split("\n"));
}

/**
 * The href, when following it is safe.
 *
 * A description is remote text, so only a scheme a browser would open survives;
 * everything else — `javascript:`, a repo-relative path that means nothing on
 * this machine — is rendered as the characters it is.
 */
export function safeHref(href: string): string | null {
  const trimmed = href.trim();
  return /^(?:https?:\/\/|mailto:)\S+$/i.test(trimmed) ? trimmed : null;
}

function parseBlocks(lines: string[]): MdBlock[] {
  const blocks: MdBlock[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index];
    if (line.trim() === "") {
      index += 1;
      continue;
    }

    // A comment is the author's note to the next author; a forge host hides it.
    // An unclosed one hides the rest of the document, as it does in a browser.
    if (COMMENT_OPEN.test(line)) {
      while (index < lines.length && !lines[index].includes("-->")) index += 1;
      const closing = index < lines.length ? lines[index] : "";
      const after = closing.slice(closing.indexOf("-->") + 3).trim();
      index += 1;
      if (after !== "") blocks.push({ kind: "paragraph", spans: inlineSpans(after) });
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[1] ?? "```";
      const body: string[] = [];
      index += 1;
      while (index < lines.length && !closesFence(lines[index], marker)) {
        body.push(lines[index]);
        index += 1;
      }
      // Past the closing fence, or past the end when the body never closed it.
      index += 1;
      blocks.push({ kind: "code", lang: fence[2] || null, text: body.join("\n") });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      const level = (heading[1] ?? "#").length;
      blocks.push({ kind: "heading", level, spans: inlineSpans(heading[2] ?? "") });
      index += 1;
      continue;
    }

    if (RULE.test(line)) {
      blocks.push({ kind: "rule" });
      index += 1;
      continue;
    }

    if (QUOTE.test(line)) {
      const inner: string[] = [];
      while (index < lines.length && QUOTE.test(lines[index])) {
        inner.push(QUOTE.exec(lines[index])?.[1] ?? "");
        index += 1;
      }
      blocks.push({ kind: "quote", blocks: parseBlocks(inner) });
      continue;
    }

    // A table is only a table with its delimiter row: one `|` in a sentence is
    // a pipe, and drawing a one-column grid around it loses the sentence.
    if (line.includes("|") && index + 1 < lines.length && isDelimiterRow(lines[index + 1])) {
      const head = tableCells(line).map(inlineSpans);
      const rows: MdSpan[][][] = [];
      index += 2;
      while (index < lines.length && lines[index].includes("|")) {
        rows.push(tableCells(lines[index]).map(inlineSpans));
        index += 1;
      }
      blocks.push({ kind: "table", head, rows });
      continue;
    }

    const first = ITEM.exec(line);
    if (first) {
      const ordered = first[3] !== undefined;
      const raw: { depth: number; checked: boolean | null; text: string }[] = [];
      while (index < lines.length) {
        const item = ITEM.exec(lines[index]);
        if (item) {
          // A bullet list under a numbered one is a second list, not a row of
          // this one: they are numbered differently and must not share a count.
          if ((item[3] !== undefined) !== ordered) break;
          raw.push(rawItem(item));
          index += 1;
          continue;
        }
        const current = raw.at(-1);
        if (current === undefined || lines[index].trim() === "" || startsBlock(lines[index])) break;
        current.text += `\n${lines[index].trim()}`;
        index += 1;
      }
      blocks.push({
        kind: "list",
        ordered,
        items: raw.map((entry) => ({
          depth: entry.depth,
          checked: entry.checked,
          spans: inlineSpans(entry.text),
        })),
      });
      continue;
    }

    // Soft breaks are kept as newlines rather than collapsed to spaces: a forge
    // host renders them as breaks, and a wrapped checklist reads as one line
    // per item there and must here too.
    const text = [line.trim()];
    index += 1;
    while (index < lines.length && lines[index].trim() !== "" && !startsBlock(lines[index])) {
      text.push(lines[index].trim());
      index += 1;
    }
    // A line that was only a layout tag leaves nothing, and must leave no gap.
    const spans = trimSpans(inlineSpans(text.join("\n")));
    if (spans.length > 0) blocks.push({ kind: "paragraph", spans });
  }

  return blocks;
}

function startsBlock(line: string): boolean {
  return (
    FENCE.test(line) ||
    HEADING.test(line) ||
    RULE.test(line) ||
    QUOTE.test(line) ||
    ITEM.test(line) ||
    COMMENT_OPEN.test(line)
  );
}

/** A paragraph without the blank edges its dropped wrapper tags left behind. */
function trimSpans(spans: MdSpan[]): MdSpan[] {
  const blank = (span: MdSpan | undefined): boolean =>
    span?.kind === "text" && span.text.trim() === "";
  const out = [...spans];
  while (blank(out[0])) out.shift();
  while (blank(out.at(-1))) out.pop();
  const first = out[0];
  if (first?.kind === "text") out[0] = { ...first, text: first.text.trimStart() };
  const last = out.at(-1);
  if (last?.kind === "text") out[out.length - 1] = { ...last, text: last.text.trimEnd() };
  return out;
}

function closesFence(line: string, marker: string): boolean {
  const trimmed = line.trim();
  return trimmed.length >= marker.length && [...trimmed].every((char) => char === marker[0]);
}

function rawItem(match: RegExpExecArray): { depth: number; checked: boolean | null; text: string } {
  const indent = (match[1] ?? "").replace(/\t/g, "  ").length;
  const body = match[4] ?? "";
  const task = TASK.exec(body);
  return {
    depth: Math.min(Math.floor(indent / 2), MAX_DEPTH),
    checked: task === null ? null : (task[1] ?? " ").toLowerCase() === "x",
    text: task === null ? body : (task[2] ?? ""),
  };
}

function tableCells(line: string): string[] {
  return line
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((cell) => cell.trim());
}

function isDelimiterRow(line: string): boolean {
  if (!line.includes("|")) return false;
  const cells = tableCells(line);
  return cells.length > 0 && cells.every((cell) => /^:?-+:?$/.test(cell));
}

/** One line of prose, split into the runs a renderer draws differently. */
export function inlineSpans(text: string): MdSpan[] {
  return spansWith(text, false, false);
}

function spansWith(text: string, strong: boolean, em: boolean): MdSpan[] {
  const spans: MdSpan[] = [];
  let plain = "";
  let index = 0;

  const flush = (): void => {
    if (plain !== "") {
      spans.push({ kind: "text", text: plain, strong, em });
      plain = "";
    }
  };

  while (index < text.length) {
    const char = text[index];
    const rest = text.slice(index);

    if (char === "`") {
      const ticks = /^`+/.exec(rest)?.[0] ?? "`";
      const close = text.indexOf(ticks, index + ticks.length);
      if (close !== -1) {
        flush();
        spans.push({ kind: "code", text: text.slice(index + ticks.length, close).trim() });
        index = close + ticks.length;
        continue;
      }
    }

    if (char === "!") {
      const image = IMAGE.exec(rest);
      if (image !== null) {
        flush();
        spans.push(imageSpan(image[1] ?? "", image[2] ?? "", null, null));
        index += image[0].length;
        continue;
      }
    }

    if (char === "[") {
      // A badge: the image is the link's text, and `[^\]]*` below would cut
      // the link at the image's own `]`.
      const linked = LINKED_IMAGE.exec(rest);
      if (linked !== null) {
        flush();
        spans.push(imageSpan(linked[1] ?? "", linked[2] ?? "", safeHref(linked[3] ?? ""), null));
        index += linked[0].length;
        continue;
      }
      const link = /^\[([^\]]*)\]\(\s*([^\s)]+)(?:\s+"[^"]*")?\s*\)/.exec(rest);
      const href = link === null ? null : safeHref(link[2] ?? "");
      if (link !== null && href !== null) {
        flush();
        spans.push({ kind: "link", text: link[1] || href, href });
        index += link[0].length;
        continue;
      }
    }

    if (char === "<") {
      const comment = HTML_COMMENT.exec(rest);
      if (comment !== null) {
        index += comment[0].length;
        continue;
      }
      const tag = HTML_TAG.exec(rest);
      const name = tag?.[2]?.toLowerCase() ?? "";
      if (tag !== null && name === "img" && tag[1] === "") {
        const attrs = tag[3] ?? "";
        const src = attribute(attrs, "src");
        if (src !== null) {
          flush();
          spans.push(imageSpan(attribute(attrs, "alt") ?? "", src, null, cssWidth(attrs)));
          index += tag[0].length;
          continue;
        }
      }
      if (tag !== null && name === "br") {
        plain += "\n";
        index += tag[0].length;
        continue;
      }
      if (tag !== null && LAYOUT_TAGS.has(name)) {
        index += tag[0].length;
        continue;
      }
      const auto = /^<([^\s>]+)>/.exec(rest);
      const href = auto === null ? null : safeHref(auto[1] ?? "");
      if (auto !== null && href !== null) {
        flush();
        spans.push({ kind: "link", text: href, href });
        index += auto[0].length;
        continue;
      }
    }

    if (char === "h") {
      const bare = /^https?:\/\/[^\s<>"'`)\]]+/.exec(rest)?.[0];
      if (bare !== undefined) {
        // Trailing punctuation belongs to the sentence, not to the address.
        const href = bare.replace(/[.,;:!?]+$/, "");
        flush();
        spans.push({ kind: "link", text: href, href });
        index += href.length;
        continue;
      }
    }

    if (char === "*" || char === "_") {
      const marker = text.startsWith(char + char, index) ? char + char : char;
      const close = closingMarker(text, index, marker);
      if (close !== -1) {
        flush();
        const bold = marker.length === 2;
        const inner = text.slice(index + marker.length, close);
        spans.push(...spansWith(inner, strong || bold, em || !bold));
        index = close + marker.length;
        continue;
      }
    }

    plain += char;
    index += 1;
  }

  flush();
  return spans;
}

function imageSpan(alt: string, src: string, href: string | null, width: string | null): MdImage {
  // `![a](<my shot.png>)`: the brackets are how a destination holds a space.
  const bare = src.startsWith("<") && src.endsWith(">") ? src.slice(1, -1) : src;
  return { kind: "image", alt, src: bare.trim(), href, width };
}

/** One attribute of an HTML tag, quoted or not. `name` is always a literal. */
function attribute(attrs: string, name: string): string | null {
  const match = new RegExp(
    String.raw`(?:^|\s)${name}\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))`,
    "i",
  ).exec(attrs);
  return match === null ? null : (match[1] ?? match[2] ?? match[3] ?? null);
}

/** `<img width>` as a CSS length — a bare number is pixels — or `null`. */
function cssWidth(attrs: string): string | null {
  const width = attribute(attrs, "width")?.trim() ?? "";
  if (/^\d{1,5}$/.test(width)) return `${width}px`;
  return /^\d{1,5}(?:px|%)$/.test(width) ? width : null;
}

/**
 * Where `marker` closes the emphasis it opened at `open`, or -1.
 *
 * The word-boundary guard is what keeps `PAYIN_CO_NEQUI` out of italics: an
 * underscore between two word characters is part of a name. `*` deliberately
 * has no such guard — GFM allows it inside a word.
 */
function closingMarker(text: string, open: number, marker: string): number {
  const opensWord = marker[0] === "_" && WORD.test(text[open - 1] ?? "");
  const after = text[open + marker.length] ?? "";
  if (opensWord || after === "" || /\s/.test(after)) return -1;

  let at = text.indexOf(marker, open + marker.length + 1);
  while (at !== -1) {
    const before = text[at - 1] ?? "";
    const next = text[at + marker.length] ?? "";
    if (!/\s/.test(before) && (marker[0] !== "_" || !WORD.test(next))) return at;
    at = text.indexOf(marker, at + 1);
  }
  return -1;
}
