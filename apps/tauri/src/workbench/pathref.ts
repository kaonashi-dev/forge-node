// Recognising a file that terminal output or an agent's prose just named.
//
// Pure and filesystem-free: this decides what *looks* like a path and what it
// means relative to the checkout. Whether the file is there is the daemon's
// answer; the index here is only what a file tree already read.

import type { FileEntry } from "./types";

/** One reference, as offsets into the text it was found in. */
export type PathRef = {
  from: number;
  /** One past the last character, so `text.slice(from, to)` is the reference. */
  to: number;
  /** Checkout-relative, which is the only spelling `open_file` takes. */
  path: string;
  /** The line a `:12` or `#L12` suffix named. */
  line: number | null;
};

/**
 * Characters that cannot be inside a path in output.
 *
 * `:` and `#` are absent because they carry the line suffix, and `.`, `-`,
 * `_`, `~`, `@` and `+` are ordinary in a filename.
 */
const SEPARATORS = new Set([
  " ",
  "\t",
  '"',
  "'",
  "`",
  "(",
  ")",
  "[",
  "]",
  "{",
  "}",
  "<",
  ">",
  ",",
  ";",
  "|",
  "&",
  "=",
  "*",
  " ",
]);

/** Sentence punctuation a path can pick up at the end of a clause. */
const TRAILING = /[.,;:!?'"]+$/;
const ANCHOR = /^(.+?)#L(\d+)$/i;
/** `path:12` and `path:12:5`; the column is dropped, the editor takes a line. */
const POSITION = /^(.+?):(\d+)(?::\d+)?$/;
/** Written as a path rather than guessed to be one. */
const EXPLICIT = /^(\.\/|\/|~\/|file:\/\/)/;

/** Every reference in one line of text, in reading order. */
export function findPathRefs(text: string, root: string | null): PathRef[] {
  const refs: PathRef[] = [];
  let at = 0;
  while (at < text.length) {
    if (SEPARATORS.has(text[at] ?? "")) {
      at += 1;
      continue;
    }
    let end = at;
    while (end < text.length && !SEPARATORS.has(text[end] ?? "")) end += 1;
    const ref = parseToken(text.slice(at, end), at, root);
    if (ref) refs.push(ref);
    at = end;
  }
  return refs;
}

/** The reference covering a character offset, for a click or a hover. */
export function refAt(refs: readonly PathRef[], index: number): PathRef | null {
  return refs.find((ref) => index >= ref.from && index < ref.to) ?? null;
}

function parseToken(raw: string, at: number, root: string | null): PathRef | null {
  const trimmed = raw.replace(TRAILING, "");
  if (trimmed === "" || trimmed.startsWith("-")) return null;

  let body = trimmed;
  let line: number | null = null;
  const anchor = ANCHOR.exec(body);
  const position = anchor ? null : POSITION.exec(body);
  if (anchor) {
    body = anchor[1] ?? body;
    line = Number(anchor[2]);
  } else if (position) {
    body = position[1] ?? body;
    line = Number(position[2]);
  }

  // A URL is somebody else's link: `openUrl` opens those, and a scheme this
  // side does not know is not a file either.
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(body) && !body.startsWith("file://")) return null;

  const explicit = EXPLICIT.test(body);
  const path = relativize(body, root);
  if (path === null) return null;
  if (!explicit && !looksLikeFile(path)) return null;
  return { from: at, to: at + trimmed.length, path, line };
}

/**
 * A guessed path needs an extension to be one.
 *
 * Without this every `cargo check/clippy`, `@scope/package` and `1.4.1` in a
 * sentence becomes a link. The two-character floor is what keeps `e.g` out
 * while `README.md` stays in; a token with a slash has already shown its hand,
 * so one character is enough there.
 */
function looksLikeFile(path: string): boolean {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  if (dot < 0) return false;
  const extension = name.slice(dot + 1);
  if (!/^[A-Za-z][A-Za-z0-9]{0,9}$/.test(extension)) return false;
  return path.includes("/") || extension.length >= 2;
}

/**
 * Rewrite a reference as a checkout-relative path, or reject it.
 *
 * Anything outside the checkout is rejected rather than opened: the workbench
 * reads through `open_file`, which takes a workspace and a path inside it.
 */
function relativize(reference: string, root: string | null): string | null {
  let value = reference;
  if (value.startsWith("file://")) value = value.slice("file://".length);
  while (value.startsWith("./")) value = value.slice(2);

  if (value.startsWith("~/")) {
    return root === null ? null : underRoot(segments(value.slice(2)), root);
  }
  if (value.startsWith("/")) {
    if (root === null) return null;
    return value.startsWith(`${root}/`) ? clean(value.slice(root.length + 1)) : null;
  }
  return clean(value);
}

function segments(path: string): string[] {
  return path.split("/").filter((part) => part !== "");
}

function clean(path: string): string | null {
  const parts = segments(path);
  if (parts.length === 0 || parts.includes("..")) return null;
  return parts.join("/");
}

/**
 * Read a `~`-spelled path against the checkout without knowing `$HOME`.
 *
 * The longest tail of the checkout path the reference opens with is the same
 * directory said from home — `~/dev/forge-node/apps` under a checkout at
 * `/Users/x/dev/forge-node` is `apps`. Longest tail first, so a project whose
 * own name repeats a parent segment does not match on the short one.
 */
function underRoot(parts: string[], root: string): string | null {
  const rootParts = segments(root);
  for (let take = Math.min(rootParts.length, parts.length); take > 0; take -= 1) {
    const tail = rootParts.slice(rootParts.length - take);
    if (tail.every((part, index) => parts[index] === part)) {
      return clean(parts.slice(take).join("/"));
    }
  }
  return null;
}

/** What a loaded file tree can answer about a guessed path. */
export type PathIndex = {
  files: ReadonlySet<string>;
  /** Basename to the files carrying it, for a reference that named no directory. */
  byName: ReadonlyMap<string, string[]>;
};

export function buildPathIndex(entries: readonly FileEntry[]): PathIndex {
  const files = new Set<string>();
  const byName = new Map<string, string[]>();
  for (const entry of entries) {
    if (entry.kind === "Directory") continue;
    files.add(entry.path);
    const name = entry.path.slice(entry.path.lastIndexOf("/") + 1);
    const bucket = byName.get(name);
    if (bucket) bucket.push(entry.path);
    else byName.set(name, [entry.path]);
  }
  return { files, byName };
}

/**
 * The file a reference names, or `null` when the listing says there is none.
 *
 * A missing index means "nothing has read this checkout", not "no such file":
 * the candidate goes through and the daemon answers. An ambiguous basename is
 * never guessed at — one wrong jump reads as a fact.
 */
export function resolvePath(candidate: string, index: PathIndex | null): string | null {
  if (index === null) return candidate;
  if (index.files.has(candidate)) return candidate;

  // `git diff` spells the same file `a/src/x.rs` and `b/src/x.rs`.
  const undiffed = candidate.replace(/^[ab]\//, "");
  if (undiffed !== candidate && index.files.has(undiffed)) return undiffed;

  const name = candidate.slice(candidate.lastIndexOf("/") + 1);
  const named = index.byName.get(name) ?? [];
  if (candidate.includes("/")) {
    const matches = named.filter((path) => path.endsWith(`/${undiffed}`));
    return matches.length === 1 ? (matches[0] ?? null) : null;
  }
  return named.length === 1 ? (named[0] ?? null) : null;
}

export type TextPiece = { text: string; ref: PathRef | null };

/** One line split into plain runs and reference runs, for a DOM surface. */
export function splitPathRefs(text: string, refs: readonly PathRef[]): TextPiece[] {
  if (refs.length === 0) return [{ text, ref: null }];
  const pieces: TextPiece[] = [];
  let at = 0;
  for (const ref of refs) {
    if (ref.from > at) pieces.push({ text: text.slice(at, ref.from), ref: null });
    pieces.push({ text: text.slice(ref.from, ref.to), ref });
    at = ref.to;
  }
  if (at < text.length) pieces.push({ text: text.slice(at), ref: null });
  return pieces;
}
