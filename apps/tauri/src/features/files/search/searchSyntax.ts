// Colour bounded excerpts without loading files or treating source text as HTML.
import { createLowlight } from "lowlight";
import bash from "highlight.js/lib/languages/bash";
import cpp from "highlight.js/lib/languages/cpp";
import css from "highlight.js/lib/languages/css";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import makefile from "highlight.js/lib/languages/makefile";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { hitSegments } from "./fileContentSearch";
import type { EditorScopes } from "../../../theme/editorTheme";

const highlighter = createLowlight({
  bash,
  cpp,
  css,
  go,
  ini,
  java,
  javascript,
  json,
  kotlin,
  markdown,
  makefile,
  python,
  rust,
  sql,
  typescript,
  xml,
  yaml,
});

const LANGUAGES: Readonly<Record<string, string>> = {
  rs: "rust",
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  jsonc: "json",
  py: "python",
  pyi: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  c: "cpp",
  h: "cpp",
  cc: "cpp",
  cpp: "cpp",
  hpp: "cpp",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  ini: "ini",
  cfg: "ini",
  env: "ini",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  mk: "makefile",
  make: "makefile",
  prisma: "java",
  css: "css",
  html: "xml",
  htm: "xml",
  xhtml: "xml",
  xml: "xml",
  svg: "xml",
  md: "markdown",
  markdown: "markdown",
  sql: "sql",
};

const SCOPES: Readonly<Record<string, keyof EditorScopes>> = {
  comment: "comment",
  quote: "comment",
  keyword: "keyword",
  operator: "operator",
  punctuation: "punctuation",
  variable: "variable",
  property: "property",
  attr: "property",
  attribute: "attribute",
  title: "function",
  type: "type",
  built_in: "type",
  string: "string",
  char: "string",
  regexp: "regexp",
  number: "number",
  literal: "constant",
  symbol: "constant",
  tag: "tag",
  name: "tag",
  selector_tag: "tag",
  selector_class: "type",
  selector_id: "constant",
  section: "heading",
  link: "link",
  meta: "meta",
};

export type SyntaxToken = { text: string; scope: keyof EditorScopes | null };
export type SyntaxHit = { hit: boolean; tokens: SyntaxToken[] };
type SyntaxNode = ReturnType<typeof highlighter.highlight>["children"][number];

// UTF-16 units, checked before joining: minified hits must not stall a search.
export const MAX_SYNTAX_EXCERPT_LENGTH = 32_768;

/** Each contiguous excerpt starts fresh; syntax state must not cross omitted lines. */
export function highlightExcerpt(path: string, lines: readonly string[]): SyntaxToken[][] {
  const plain = () => lines.map((text) => [{ text, scope: null }]);
  const name = path.split("/").at(-1)?.toLowerCase() ?? "";
  const language =
    name === ".env" || name.startsWith(".env.")
      ? "ini"
      : name === "makefile" || name === "gnumakefile"
        ? "makefile"
        : LANGUAGES[name.split(".").at(-1) ?? ""];
  if (!language) return plain();
  let size = 0;
  for (const line of lines) {
    size += line.length + 1;
    if (size > MAX_SYNTAX_EXCERPT_LENGTH) return plain();
  }
  if (lines.length === 0) return [];

  try {
    const tree = highlighter.highlight(language, lines.join("\n"));
    const rows: SyntaxToken[][] = [[]];
    function visit(node: SyntaxNode, inherited: keyof EditorScopes | null): void {
      if (node.type === "text") {
        const parts = node.value.split("\n");
        for (let i = 0; i < parts.length; i += 1) {
          if (i > 0) rows.push([]);
          const text = parts[i];
          if (!text) continue;
          const row = rows[rows.length - 1];
          const last = row.at(-1);
          if (last?.scope === inherited) last.text += text;
          else row.push({ text, scope: inherited });
        }
      } else if (node.type === "element") {
        const classes = node.properties.className;
        let scope = inherited;
        if (Array.isArray(classes)) {
          for (const name of classes) {
            if (typeof name === "string" && name.startsWith("hljs-")) {
              scope = SCOPES[name.slice(5)] ?? scope;
            }
          }
          if (classes.includes("class_")) scope = "type";
          if (classes.includes("function_")) scope = "function";
        }
        for (const child of node.children) visit(child, scope);
      }
    }
    for (const node of tree.children) visit(node, null);
    return rows;
  } catch {
    // A partial source fragment must remain readable if its grammar rejects it.
    return plain();
  }
}

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
