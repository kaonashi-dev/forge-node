// Which grammar a file gets highlighted with.
//
// The daemon already answers `ReadFile` with a `language` hint, so that is
// what is believed first — one place decides what a `.rs` is. Its table is
// short, though (`fs-service`'s `language_for`), and it says nothing about a
// stylesheet or a shell script, so the extension is read as a fallback rather
// than a file being left grey for want of four characters here.
//
// Anything still unknown highlights as nothing at all. A wrong grammar does
// not degrade to plain text — it paints confident, wrong colour over half a
// file, which is worse than no colour.

/**
 * The extension, lowercased, or `""`.
 *
 * A leading dot is part of the name and not an extension: `.gitignore` is a
 * dotfile, not a `gitignore` file, and a grammar chosen from `gitignore` would
 * paint confident, wrong colour over a config.
 */
export function extensionOf(name: string): string {
  const cut = name.lastIndexOf(".");
  if (cut <= 0) return "";
  return name.slice(cut + 1).toLowerCase();
}

/** The grammars this app loads. Each one is a lazily imported chunk. */
export type GrammarId =
  | "typescript"
  | "tsx"
  | "javascript"
  | "python"
  | "rust"
  | "json"
  | "toml"
  | "yaml"
  | "markdown"
  | "css"
  | "html"
  | "shellscript";

/** What the daemon's `language` hint maps to. */
const FROM_DAEMON: Record<string, GrammarId> = {
  rust: "rust",
  javascript: "javascript",
  python: "python",
  typescript: "typescript",
  tsx: "tsx",
  json: "json",
  toml: "toml",
  markdown: "markdown",
};

/** What an extension maps to, when the daemon had nothing to say. */
const FROM_EXTENSION: Record<string, GrammarId> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  jsx: "tsx",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  py: "python",
  pyi: "python",
  pyw: "python",
  rs: "rust",
  json: "json",
  jsonc: "json",
  toml: "toml",
  yml: "yaml",
  yaml: "yaml",
  md: "markdown",
  mdx: "markdown",
  markdown: "markdown",
  css: "css",
  html: "html",
  htm: "html",
  sh: "shellscript",
  bash: "shellscript",
  zsh: "shellscript",
};

/** Files that carry their language in the whole name rather than a suffix. */
const FROM_NAME: Record<string, GrammarId> = {
  dockerfile: "shellscript",
  makefile: "shellscript",
  ".zshrc": "shellscript",
  ".bashrc": "shellscript",
  ".bash_profile": "shellscript",
  ".gitignore": "shellscript",
};

function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/**
 * The grammar for one file, or `null` to leave it unpainted.
 *
 * `hint` is `FileContents.language`; `path` is the workspace-relative path.
 */
export function grammarFor(hint: string, path: string): GrammarId | null {
  const fromDaemon = FROM_DAEMON[hint];
  if (fromDaemon) return fromDaemon;

  const name = basename(path);
  const fromName = FROM_NAME[name.toLowerCase()];
  if (fromName) return fromName;

  return FROM_EXTENSION[extensionOf(name)] ?? null;
}
