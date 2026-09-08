// A3: which CodeMirror language a `GrammarId` loads.
//
// `workbench/language.ts` still answers *which* grammar a file gets — that
// table reads the daemon's hint first and the extension second, and none of
// that changes with the editor underneath it. This module is the other half:
// what that identifier loads now that the answer is a Lezer parser rather than
// a TextMate grammar.
//
// Every entry is a dynamic import, so a window that never opens a `.rs` never
// pays for the Rust parser. An unknown grammar loads nothing and the file is
// edited as plain text, which is what it was before any of this existed.

import type { LanguageSupport, StreamParser } from "@codemirror/language";
import type { GrammarId } from "../language";

type Loader = () => Promise<LanguageSupport>;

/**
 * Lezer has no first-party TOML or shell grammar.
 *
 * `@codemirror/legacy-modes` covers both through `StreamLanguage`, which is
 * CodeMirror 5's line-at-a-time tokeniser rather than a real parse tree: it
 * colours correctly and gives up folding and structural selection. That is the
 * trade `plan-ui-ux.md` §8.2 accepted, and it is why these two go through a
 * different door from the rest.
 */
async function fromLegacyMode(
  mode: () => Promise<StreamParser<unknown>>,
): Promise<LanguageSupport> {
  const [{ LanguageSupport, StreamLanguage }, parser] = await Promise.all([
    import("@codemirror/language"),
    mode(),
  ]);
  return new LanguageSupport(StreamLanguage.define(parser));
}

const LOADERS: Record<GrammarId, Loader> = {
  typescript: async () =>
    (await import("@codemirror/lang-javascript")).javascript({ typescript: true }),
  tsx: async () =>
    (await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: true }),
  javascript: async () => (await import("@codemirror/lang-javascript")).javascript(),
  rust: async () => (await import("@codemirror/lang-rust")).rust(),
  json: async () => (await import("@codemirror/lang-json")).json(),
  css: async () => (await import("@codemirror/lang-css")).css(),
  html: async () => (await import("@codemirror/lang-html")).html(),
  markdown: async () => (await import("@codemirror/lang-markdown")).markdown(),
  yaml: async () => (await import("@codemirror/lang-yaml")).yaml(),
  toml: () => fromLegacyMode(async () => (await import("@codemirror/legacy-modes/mode/toml")).toml),
  shellscript: () =>
    fromLegacyMode(async () => (await import("@codemirror/legacy-modes/mode/shell")).shell),
};

/**
 * A parser can only be loaded once.
 *
 * Two editor tabs on two `.ts` files must share one `LanguageSupport`: the
 * import is memoised, not the extension, because CM6 is happy to install the
 * same support object into several views.
 */
const loaded = new Map<GrammarId, Promise<LanguageSupport>>();

export function languageFor(grammar: GrammarId | null): Promise<LanguageSupport> | null {
  if (!grammar) return null;
  const cached = loaded.get(grammar);
  if (cached) return cached;
  const pending = LOADERS[grammar]().catch((error: unknown) => {
    // A failed chunk must not poison every later open of the same language.
    loaded.delete(grammar);
    throw error;
  });
  loaded.set(grammar, pending);
  return pending;
}

/** The grammars that have a CM6 loader, for the registry test. */
export function loadableGrammars(): GrammarId[] {
  return Object.keys(LOADERS) as GrammarId[];
}
