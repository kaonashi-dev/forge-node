// Recognising a file that terminal output or an agent's prose just named.
//
// Pure and filesystem-free: this decides what *looks* like a path and what it
// means relative to the checkout. Whether the file is there is the daemon's
// answer; the index here is only what the `ListFiles` navigation listing already returned.

import type { FileEntry } from "../../../contracts/workbench";

/** One reference, as offsets into the text it was found in. */
export type PathRef = {
  from: number;
  /** One past the last character, so `text.slice(from, to)` is the reference. */
  to: number;
  /** Checkout-relative, which is the only spelling `open_file` takes. */
  path: string;
  /** The line a `:12` or `#L12` suffix named. */
  line: number | null;
  /**
   * Absolute path of a checkout other than the active one.
   *
   * `null` means the reference is relative to the root that was asked about.
   * A `~/` path into a worktree names that worktree, not the window's checkout.
   */
  checkout: string | null;
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

/**
 * Every reference in one line of text, in reading order.
 *
 * `extraRoots` are other checkouts a `~/` or absolute path may name. The
 * longest match wins, so a worktree beats the directory it sits beside.
 */
export function findPathRefs(
  text: string,
  root: string | null,
  extraRoots: readonly string[] = [],
): PathRef[] {
  const refs: PathRef[] = [];
  let at = 0;
  while (at < text.length) {
    if (SEPARATORS.has(text[at] ?? "")) {
      at += 1;
      continue;
    }
    let end = at;
    while (end < text.length && !SEPARATORS.has(text[end] ?? "")) end += 1;
    const ref = parseToken(text.slice(at, end), at, root, extraRoots);
    if (ref) refs.push(ref);
    at = end;
  }
  return refs;
}

/** The reference covering a character offset, for a click or a hover. */
export function refAt(refs: readonly PathRef[], index: number): PathRef | null {
  return refs.find((ref) => index >= ref.from && index < ref.to) ?? null;
}

function parseToken(
  raw: string,
  at: number,
  root: string | null,
  extraRoots: readonly string[],
): PathRef | null {
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
  const placed = place(body, root, extraRoots);
  if (placed === null) return null;
  if (!explicit && !looksLikeFile(placed.path)) return null;
  return {
    from: at,
    to: at + trimmed.length,
    path: placed.path,
    line,
    checkout: placed.checkout,
  };
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

type Placement = { path: string; checkout: string | null };

/**
 * Rewrite a reference as a checkout-relative path, or reject it.
 *
 * A relative path stays on `root`. A `~/` or absolute path may name another
 * checkout in `extraRoots`; the workbench can only open a path inside one it
 * knows, so anything outside all of them is rejected.
 */
function place(
  reference: string,
  root: string | null,
  extraRoots: readonly string[],
): Placement | null {
  let value = reference;
  if (value.startsWith("file://")) value = value.slice("file://".length);
  while (value.startsWith("./")) value = value.slice(2);

  const primary = root === null ? null : normalizeRoot(root);
  const extras: string[] = [];
  for (const extra of extraRoots) {
    const normalized = normalizeRoot(extra);
    if (normalized !== "" && normalized !== primary && !extras.includes(normalized)) {
      extras.push(normalized);
    }
  }

  if (value.startsWith("~/")) return placeHome(segments(value.slice(2)), primary, extras);
  if (value.startsWith("/")) return placeAbsolute(value, primary, extras);
  const path = clean(value);
  return path === null ? null : { path, checkout: null };
}

function normalizeRoot(root: string): string {
  return root.length > 1 ? root.replace(/\/+$/, "") : root;
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
 * own name repeats a parent segment does not match on the short one. An empty
 * remainder means the reference named the checkout itself, which is not a file.
 */
function underRoot(parts: string[], root: string): { path: string | null; score: number } | null {
  const rootParts = segments(root);
  for (let take = Math.min(rootParts.length, parts.length); take > 0; take -= 1) {
    const tail = rootParts.slice(rootParts.length - take);
    if (tail.every((part, index) => parts[index] === part)) {
      return { path: clean(parts.slice(take).join("/")), score: take };
    }
  }
  return null;
}

function placeHome(
  parts: string[],
  primary: string | null,
  extras: readonly string[],
): Placement | null {
  const roots: { root: string; checkout: string | null }[] = [];
  if (primary) roots.push({ root: primary, checkout: null });
  for (const extra of extras) roots.push({ root: extra, checkout: extra });

  let best: { path: string | null; checkout: string | null; score: number } | null = null;
  for (const { root, checkout } of roots) {
    const placed = underRoot(parts, root);
    if (!placed || (best !== null && placed.score <= best.score)) continue;
    best = { path: placed.path, checkout, score: placed.score };
  }
  if (best === null || best.path === null) return null;
  return { path: best.path, checkout: best.checkout };
}

function placeAbsolute(
  value: string,
  primary: string | null,
  extras: readonly string[],
): Placement | null {
  const roots: { root: string; checkout: string | null }[] = [];
  if (primary) roots.push({ root: primary, checkout: null });
  for (const extra of extras) roots.push({ root: extra, checkout: extra });

  let best: { path: string; checkout: string | null; score: number } | null = null;
  for (const { root, checkout } of roots) {
    const prefix = `${root}/`;
    if (!value.startsWith(prefix)) continue;
    const path = clean(value.slice(prefix.length));
    if (path === null) continue;
    const score = segments(root).length;
    if (best !== null && score <= best.score) continue;
    best = { path, checkout, score };
  }
  return best === null ? null : { path: best.path, checkout: best.checkout };
}

/** What a loaded file tree can answer about a guessed path. */
export type PathIndex = {
  files: ReadonlySet<string>;
  directories: ReadonlySet<string>;
  complete: boolean;
  /** Basename to the files carrying it, for a reference that named no directory. */
  byName: ReadonlyMap<string, string[]>;
};

export function buildPathIndex(entries: readonly FileEntry[], complete = true): PathIndex {
  const files = new Set<string>();
  const directories = new Set<string>();
  const byName = new Map<string, string[]>();
  for (const entry of entries) {
    if (entry.kind === "Directory") {
      directories.add(entry.path);
      continue;
    }
    if (entry.kind !== "File") continue;
    files.add(entry.path);
    const name = entry.path.slice(entry.path.lastIndexOf("/") + 1);
    const bucket = byName.get(name);
    if (bucket) bucket.push(entry.path);
    else byName.set(name, [entry.path]);
  }
  return { files, directories, byName, complete };
}

/**
 * The file a reference names, or `null` when the listing says there is none.
 *
 * A missing index means "nothing has read this checkout", not "no such file":
 * the candidate goes through and the daemon answers. A slash path the listing
 * has not seen yet goes through too — an agent names the file in the same
 * breath it creates it, and a complete index from a moment ago is not a reason
 * to refuse the click. An ambiguous basename is never guessed at.
 */
export function resolvePath(candidate: string, index: PathIndex | null): string | null {
  if (index === null) return candidate;
  if (index.files.has(candidate)) return candidate;
  if (index.directories.has(candidate)) return null;

  // `git diff` spells the same file `a/src/x.rs` and `b/src/x.rs`.
  const undiffed = candidate.replace(/^[ab]\//, "");
  if (undiffed !== candidate && index.files.has(undiffed)) return undiffed;
  if (undiffed !== candidate && index.directories.has(undiffed)) return null;
  if (!index.complete && !candidate.includes("/")) return candidate;

  const name = candidate.slice(candidate.lastIndexOf("/") + 1);
  const named = index.byName.get(name) ?? [];
  if (candidate.includes("/")) {
    const matches = named.filter((path) => path.endsWith(`/${undiffed}`));
    if (matches.length === 1) return matches[0] ?? null;
    if (matches.length > 1) return null;
    return candidate;
  }
  if (!index.complete) return candidate;
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
