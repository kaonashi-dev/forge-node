import { readFileSync } from "node:fs";

export interface Node {
  role?: string;
  key?: string;
  name?: string;
  description?: string;
  value?: string;
  children?: Node[];
}

export function loadTree(path: string): Node {
  const tree = JSON.parse(readFileSync(path, "utf8")) as { accessibility?: Node };
  if (tree.accessibility === undefined) {
    throw new Error(`${path}: no "accessibility" tree`);
  }
  return tree.accessibility;
}

export function walk(node: Node): Node[] {
  return [node, ...(node.children ?? []).flatMap(walk)];
}

/** Length in code points, as Python's `len` counts a string. */
export function length(text: string): number {
  return [...text].length;
}

export function prefix(text: string, count: number): string {
  return [...text].slice(0, count).join("");
}

const NON_PRINTABLE = /[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]/u;

/** Python's `repr`, so failure messages read as they did from the Python checks. */
export function repr(value: unknown): string {
  if (value === null || value === undefined) return "None";
  if (typeof value === "boolean") return value ? "True" : "False";
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return reprString(value);
  if (Array.isArray(value)) return `[${value.map(repr).join(", ")}]`;
  if (typeof value === "object") {
    const entries = Object.entries(value).map(([k, v]) => `${reprString(k)}: ${repr(v)}`);
    return `{${entries.join(", ")}}`;
  }
  return String(value);
}

function reprString(text: string): string {
  const quote = text.includes("'") && !text.includes('"') ? '"' : "'";
  let out = quote;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (ch === "\\" || ch === quote) out += `\\${ch}`;
    else if (ch === "\t") out += "\\t";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (ch !== " " && NON_PRINTABLE.test(ch)) {
      const hex = code.toString(16);
      if (code < 0x100) out += `\\x${hex.padStart(2, "0")}`;
      else if (code < 0x10000) out += `\\u${hex.padStart(4, "0")}`;
      else out += `\\U${hex.padStart(8, "0")}`;
    } else out += ch;
  }
  return out + quote;
}

export function check(condition: boolean, message: unknown): asserts condition {
  if (condition) return;
  console.error(`AssertionError: ${typeof message === "string" ? message : repr(message)}`);
  process.exit(1);
}
