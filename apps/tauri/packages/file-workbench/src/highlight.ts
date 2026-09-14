// Minimal syntax colouring for the textarea overlay.
//
// Zed/vim-adjacent: a handful of scopes (keyword, string, comment, type,
// number, …), not a full parse tree. Wrong colour is worse than none — unknown
// grammars stay plain.

export type Scope =
  | "comment"
  | "keyword"
  | "controlKeyword"
  | "string"
  | "number"
  | "type"
  | "function"
  | "property"
  | "operator"
  | "punctuation"
  | "constant"
  | "meta"
  | "tag"
  | "attribute"
  | "plain";

export type Token = { text: string; scope: Scope };

const JS_KEYWORDS = new Set([
  "as",
  "async",
  "await",
  "break",
  "case",
  "catch",
  "class",
  "const",
  "continue",
  "debugger",
  "default",
  "delete",
  "do",
  "else",
  "enum",
  "export",
  "extends",
  "finally",
  "for",
  "from",
  "function",
  "if",
  "implements",
  "import",
  "in",
  "instanceof",
  "interface",
  "let",
  "new",
  "of",
  "package",
  "private",
  "protected",
  "public",
  "return",
  "static",
  "super",
  "switch",
  "this",
  "throw",
  "try",
  "typeof",
  "var",
  "void",
  "while",
  "with",
  "yield",
  "type",
  "namespace",
  "module",
  "declare",
  "abstract",
  "readonly",
  "keyof",
  "infer",
  "satisfies",
  "override",
]);

const JS_CONTROL = new Set([
  "if",
  "else",
  "for",
  "while",
  "do",
  "switch",
  "case",
  "break",
  "continue",
  "return",
  "throw",
  "try",
  "catch",
  "finally",
  "await",
  "yield",
]);

const JS_TYPES = new Set([
  "string",
  "number",
  "boolean",
  "any",
  "unknown",
  "never",
  "void",
  "null",
  "undefined",
  "object",
  "symbol",
  "bigint",
  "true",
  "false",
]);

const RUST_KEYWORDS = new Set([
  "as",
  "async",
  "await",
  "break",
  "const",
  "continue",
  "crate",
  "dyn",
  "else",
  "enum",
  "extern",
  "false",
  "fn",
  "for",
  "if",
  "impl",
  "in",
  "let",
  "loop",
  "match",
  "mod",
  "move",
  "mut",
  "pub",
  "ref",
  "return",
  "self",
  "Self",
  "static",
  "struct",
  "super",
  "trait",
  "true",
  "type",
  "unsafe",
  "use",
  "where",
  "while",
]);

const PYTHON_KEYWORDS = new Set([
  "and",
  "as",
  "assert",
  "async",
  "await",
  "break",
  "class",
  "continue",
  "def",
  "del",
  "elif",
  "else",
  "except",
  "False",
  "finally",
  "for",
  "from",
  "global",
  "if",
  "import",
  "in",
  "is",
  "lambda",
  "None",
  "nonlocal",
  "not",
  "or",
  "pass",
  "raise",
  "return",
  "True",
  "try",
  "while",
  "with",
  "yield",
]);

function escapeHtml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Render tokens to HTML for the highlight layer. */
export function tokensToHtml(tokens: Token[]): string {
  let out = "";
  for (const token of tokens) {
    const safe = escapeHtml(token.text);
    if (token.scope === "plain" || token.text.length === 0) {
      out += safe;
      continue;
    }
    out += `<span class="fw-tok fw-tok-${token.scope}">${safe}</span>`;
  }
  // A trailing newline keeps the last empty line's height aligned with the textarea.
  if (!out.endsWith("\n")) out += "\n";
  return out;
}

export function highlight(text: string, grammar: string | null): Token[] {
  switch (grammar) {
    case "typescript":
    case "tsx":
    case "javascript":
      return highlightCLike(text, JS_KEYWORDS, JS_CONTROL, JS_TYPES, { jsx: grammar === "tsx" });
    case "rust":
      return highlightCLike(text, RUST_KEYWORDS, RUST_KEYWORDS, new Set(["Self"]), {
        lineComment: "//",
        blockComment: ["/*", "*/"],
        hashComment: false,
      });
    case "python":
      return highlightPython(text);
    case "json":
      return highlightJson(text);
    case "css":
      return highlightCss(text);
    case "html":
      return highlightHtml(text);
    case "markdown":
      return highlightMarkdown(text);
    case "shellscript":
      return highlightShell(text);
    case "yaml":
    case "toml":
      return highlightYamlish(text);
    case "sql":
      return highlightSql(text);
    case "kotlin":
    case "prisma":
      return highlightCLike(text, JS_KEYWORDS, JS_CONTROL, JS_TYPES, {});
    default:
      return [{ text, scope: "plain" }];
  }
}

type CLikeOpts = {
  jsx?: boolean;
  lineComment?: string;
  blockComment?: [string, string];
  hashComment?: boolean;
};

function highlightCLike(
  text: string,
  keywords: Set<string>,
  control: Set<string>,
  types: Set<string>,
  opts: CLikeOpts,
): Token[] {
  const lineComment = opts.lineComment ?? "//";
  const block = opts.blockComment ?? (["/*", "*/"] as [string, string]);
  const tokens: Token[] = [];
  let i = 0;

  const push = (from: number, to: number, scope: Scope) => {
    if (to > from) tokens.push({ text: text.slice(from, to), scope });
  };

  while (i < text.length) {
    const ch = text[i]!;

    if (lineComment && text.startsWith(lineComment, i)) {
      const start = i;
      i += lineComment.length;
      while (i < text.length && text[i] !== "\n") i += 1;
      push(start, i, "comment");
      continue;
    }
    if (ch === block[0][0] && text.startsWith(block[0], i)) {
      const start = i;
      i += block[0].length;
      while (i < text.length && !text.startsWith(block[1], i)) i += 1;
      i = Math.min(text.length, i + block[1].length);
      push(start, i, "comment");
      continue;
    }
    if (ch === "#" && opts.hashComment) {
      const start = i;
      while (i < text.length && text[i] !== "\n") i += 1;
      push(start, i, "comment");
      continue;
    }

    if (ch === '"' || ch === "'" || ch === "`") {
      if (ch === "`") {
        i = readTemplate(text, i, push, (from, to) =>
          highlightCLike(text.slice(from, to), keywords, control, types, opts),
        );
        continue;
      }
      const quote = ch;
      const start = i;
      i += 1;
      while (i < text.length) {
        if (text[i] === "\\") {
          i += 2;
          continue;
        }
        if (text[i] === quote) {
          i += 1;
          break;
        }
        // A single-quoted / double-quoted string never spans lines in JS for
        // this highlighter's purposes — keeps a missing closer from eating the file.
        if (text[i] === "\n") break;
        i += 1;
      }
      push(start, i, "string");
      continue;
    }

    if (ch >= "0" && ch <= "9") {
      const start = i;
      while (i < text.length && /[\d._xa-fA-Fn]/.test(text[i]!)) i += 1;
      push(start, i, "number");
      continue;
    }

    if (/[A-Za-z_$]/.test(ch)) {
      const start = i;
      i += 1;
      while (i < text.length && /[\w$]/.test(text[i]!)) i += 1;
      const word = text.slice(start, i);
      let scope: Scope = "plain";
      if (keywords.has(word)) scope = control.has(word) ? "controlKeyword" : "keyword";
      else if (types.has(word)) scope = word === "true" || word === "false" ? "constant" : "type";
      else if (word[0] !== undefined && word[0] >= "A" && word[0] <= "Z") scope = "type";
      else {
        // `name(` → function; `name:` / `.name` → property-ish.
        let j = i;
        while (j < text.length && (text[j] === " " || text[j] === "\t")) j += 1;
        if (text[j] === "(") scope = "function";
        else if (text[start - 1] === ".") scope = "property";
      }
      push(start, i, scope);
      continue;
    }

    if ("{}[]().,;:?<>|&!=+-*/%^~".includes(ch)) {
      push(i, i + 1, /[.=<>!+\-*/%^~|&?:]/.test(ch) ? "operator" : "punctuation");
      i += 1;
      continue;
    }

    // Whitespace and anything else as plain.
    const start = i;
    i += 1;
    while (i < text.length && !/[A-Za-z_$0-9"'`/#{}[\]().,;:?<>|&!=+\-*/%^~]/.test(text[i]!)) {
      i += 1;
    }
    push(start, i, "plain");
  }

  return tokens;
}

/**
 * Read a template literal starting at the opening backtick.
 *
 * `${…}` drops into a nested highlight of the expression (brace-balanced), then
 * resumes the string. Stopping at `${` and treating the closing backtick as a
 * new opener painted the rest of the file as a string.
 */
function readTemplate(
  text: string,
  start: number,
  push: (from: number, to: number, scope: Scope) => void,
  highlightExpr: (from: number, to: number) => Token[],
): number {
  let i = start + 1;
  let chunk = start;
  while (i < text.length) {
    if (text[i] === "\\") {
      i += 2;
      continue;
    }
    if (text[i] === "`") {
      push(chunk, i + 1, "string");
      return i + 1;
    }
    if (text[i] === "$" && text[i + 1] === "{") {
      push(chunk, i, "string");
      push(i, i + 2, "punctuation");
      i += 2;
      const exprFrom = i;
      i = skipBalancedExpr(text, i);
      let offset = exprFrom;
      for (const token of highlightExpr(exprFrom, i)) {
        const to = offset + token.text.length;
        push(offset, to, token.scope);
        offset = to;
      }
      if (i < text.length && text[i] === "}") {
        push(i, i + 1, "punctuation");
        i += 1;
      }
      chunk = i;
      continue;
    }
    i += 1;
  }
  push(chunk, i, "string");
  return i;
}

/** Advance past a `${` expression body; `i` starts just after `{`, returns index of closing `}`. */
function skipBalancedExpr(text: string, i: number): number {
  let depth = 1;
  while (i < text.length && depth > 0) {
    const c = text[i]!;
    if (c === '"' || c === "'" || c === "`") {
      i = skipStringish(text, i);
      continue;
    }
    if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i += 1;
      continue;
    }
    if (c === "/" && text[i + 1] === "*") {
      i += 2;
      while (i < text.length && !(text[i] === "*" && text[i + 1] === "/")) i += 1;
      i = Math.min(text.length, i + 2);
      continue;
    }
    if (c === "{") depth += 1;
    else if (c === "}") {
      depth -= 1;
      if (depth === 0) return i;
    }
    i += 1;
  }
  return i;
}

/** Skip a `'…'`, `"…"`, or `` `…` `` (with nested interpolations) starting at `i`. */
function skipStringish(text: string, i: number): number {
  const q = text[i]!;
  i += 1;
  if (q === '"' || q === "'") {
    while (i < text.length && text[i] !== q && text[i] !== "\n") {
      if (text[i] === "\\") i += 2;
      else i += 1;
    }
    return text[i] === q ? i + 1 : i;
  }
  // Template: nest through `${…}` without counting those braces for the caller.
  while (i < text.length) {
    if (text[i] === "\\") {
      i += 2;
      continue;
    }
    if (text[i] === "`") return i + 1;
    if (text[i] === "$" && text[i + 1] === "{") {
      i += 2;
      i = skipBalancedExpr(text, i);
      if (i < text.length && text[i] === "}") i += 1;
      continue;
    }
    i += 1;
  }
  return i;
}

function highlightPython(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  const push = (from: number, to: number, scope: Scope) => {
    if (to > from) tokens.push({ text: text.slice(from, to), scope });
  };
  while (i < text.length) {
    const ch = text[i]!;
    if (ch === "#") {
      const start = i;
      while (i < text.length && text[i] !== "\n") i += 1;
      push(start, i, "comment");
      continue;
    }
    if (ch === '"' || ch === "'") {
      const triple = text.startsWith(ch + ch + ch, i);
      const start = i;
      i += triple ? 3 : 1;
      if (triple) {
        while (i < text.length && !text.startsWith(ch + ch + ch, i)) i += 1;
        i = Math.min(text.length, i + 3);
      } else {
        while (i < text.length && text[i] !== ch && text[i] !== "\n") {
          if (text[i] === "\\") i += 2;
          else i += 1;
        }
        if (text[i] === ch) i += 1;
      }
      push(start, i, "string");
      continue;
    }
    if (ch >= "0" && ch <= "9") {
      const start = i;
      while (i < text.length && /[\d._]/.test(text[i]!)) i += 1;
      push(start, i, "number");
      continue;
    }
    if (/[A-Za-z_]/.test(ch)) {
      const start = i;
      while (i < text.length && /[\w]/.test(text[i]!)) i += 1;
      const word = text.slice(start, i);
      let scope: Scope = "plain";
      if (PYTHON_KEYWORDS.has(word)) {
        scope = word === "True" || word === "False" || word === "None" ? "constant" : "keyword";
      } else {
        let j = i;
        while (j < text.length && (text[j] === " " || text[j] === "\t")) j += 1;
        if (text[j] === "(") scope = "function";
      }
      push(start, i, scope);
      continue;
    }
    push(i, i + 1, "plain");
    i += 1;
  }
  return tokens;
}

function highlightJson(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  const push = (from: number, to: number, scope: Scope) => {
    if (to > from) tokens.push({ text: text.slice(from, to), scope });
  };
  while (i < text.length) {
    const ch = text[i]!;
    if (ch === '"') {
      const start = i;
      i += 1;
      while (i < text.length && text[i] !== '"') {
        if (text[i] === "\\") i += 2;
        else i += 1;
      }
      if (text[i] === '"') i += 1;
      let j = i;
      while (j < text.length && /\s/.test(text[j]!)) j += 1;
      push(start, i, text[j] === ":" ? "property" : "string");
      continue;
    }
    if ((ch >= "0" && ch <= "9") || ch === "-") {
      const start = i;
      while (i < text.length && /[\d.eE+-]/.test(text[i]!)) i += 1;
      push(start, i, "number");
      continue;
    }
    if (text.startsWith("true", i) || text.startsWith("false", i) || text.startsWith("null", i)) {
      const word = text.startsWith("false", i)
        ? "false"
        : text.startsWith("true", i)
          ? "true"
          : "null";
      push(i, i + word.length, "constant");
      i += word.length;
      continue;
    }
    push(i, i + 1, /[{}[\],:]/.test(ch) ? "punctuation" : "plain");
    i += 1;
  }
  return tokens;
}

function highlightCss(text: string): Token[] {
  return highlightCLike(
    text,
    new Set(["@media", "@import", "@keyframes", "!important"]),
    new Set(),
    new Set(),
    {
      blockComment: ["/*", "*/"],
      lineComment: "//",
    },
  );
}

function highlightHtml(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  const push = (from: number, to: number, scope: Scope) => {
    if (to > from) tokens.push({ text: text.slice(from, to), scope });
  };
  while (i < text.length) {
    if (text.startsWith("<!--", i)) {
      const start = i;
      i = text.indexOf("-->", i);
      i = i < 0 ? text.length : i + 3;
      push(start, i, "comment");
      continue;
    }
    if (text[i] === "<") {
      const start = i;
      i += 1;
      push(start, i, "punctuation");
      if (text[i] === "/" || text[i] === "!") {
        push(i, i + 1, "punctuation");
        i += 1;
      }
      const nameStart = i;
      while (i < text.length && /[\w:-]/.test(text[i]!)) i += 1;
      push(nameStart, i, "tag");
      while (i < text.length && text[i] !== ">") {
        if (text[i] === '"' || text[i] === "'") {
          const q = text[i]!;
          const s = i;
          i += 1;
          while (i < text.length && text[i] !== q) i += 1;
          if (text[i] === q) i += 1;
          push(s, i, "string");
          continue;
        }
        if (/[A-Za-z_]/.test(text[i]!)) {
          const s = i;
          while (i < text.length && /[\w:-]/.test(text[i]!)) i += 1;
          push(s, i, "attribute");
          continue;
        }
        push(i, i + 1, "plain");
        i += 1;
      }
      if (text[i] === ">") {
        push(i, i + 1, "punctuation");
        i += 1;
      }
      continue;
    }
    const start = i;
    while (i < text.length && text[i] !== "<") i += 1;
    push(start, i, "plain");
  }
  return tokens;
}

function highlightMarkdown(text: string): Token[] {
  const tokens: Token[] = [];
  for (const line of text.split(/(?<=\n)/)) {
    if (/^#{1,6}\s/.test(line)) tokens.push({ text: line, scope: "keyword" });
    else if (/^>\s/.test(line)) tokens.push({ text: line, scope: "comment" });
    else if (line.startsWith("```")) tokens.push({ text: line, scope: "meta" });
    else tokens.push({ text: line, scope: "plain" });
  }
  return tokens;
}

function highlightShell(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  const push = (from: number, to: number, scope: Scope) => {
    if (to > from) tokens.push({ text: text.slice(from, to), scope });
  };
  while (i < text.length) {
    if (text[i] === "#") {
      const start = i;
      while (i < text.length && text[i] !== "\n") i += 1;
      push(start, i, "comment");
      continue;
    }
    if (text[i] === '"' || text[i] === "'") {
      const q = text[i]!;
      const start = i;
      i += 1;
      while (i < text.length && text[i] !== q) {
        if (text[i] === "\\" && q === '"') i += 2;
        else i += 1;
      }
      if (text[i] === q) i += 1;
      push(start, i, "string");
      continue;
    }
    push(i, i + 1, "plain");
    i += 1;
  }
  return tokens;
}

function highlightYamlish(text: string): Token[] {
  const tokens: Token[] = [];
  for (const line of text.split(/(?<=\n)/)) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith("#")) tokens.push({ text: line, scope: "comment" });
    else if (/^[\w.-]+\s*:/.test(trimmed)) {
      const indent = line.length - trimmed.length;
      const colon = trimmed.indexOf(":");
      tokens.push({ text: line.slice(0, indent), scope: "plain" });
      tokens.push({ text: trimmed.slice(0, colon), scope: "property" });
      tokens.push({ text: trimmed.slice(colon), scope: "plain" });
    } else tokens.push({ text: line, scope: "plain" });
  }
  return tokens;
}

function highlightSql(text: string): Token[] {
  const keywords = new Set(
    "select from where join left right inner outer on group by order limit insert into values update set delete create table index and or not null as in is".split(
      " ",
    ),
  );
  return highlightCLike(text, keywords, keywords, new Set(), {
    lineComment: "--",
    blockComment: ["/*", "*/"],
  });
}
