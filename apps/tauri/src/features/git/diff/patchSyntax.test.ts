import { describe, expect, it } from "vitest";
import { parsePatch } from "./patch";
import { intraLine } from "./patchDocument";
import {
  cachedPatchSyntax,
  highlightPatch,
  MAX_PATCH_SYNTAX_LENGTH,
  MAX_PATCH_SYNTAX_ROWS,
  syntaxSegments,
} from "./patchSyntax";
import type { SyntaxToken } from "../../../shared/syntax/highlight";

const textOf = (tokens: SyntaxToken[]) => tokens.map((token) => token.text).join("");

describe("patch syntax", () => {
  it("colours removed and added source without treating diff headers as code", () => {
    const rows = parsePatch(
      "diff --git a/x.ts b/x.ts\n@@ -1,1 +1,1 @@\n-const value = 1; // old\n+const value = 2; // new\n",
    );
    const syntax = highlightPatch("x.ts", rows);
    for (const [side, kind] of [
      ["before", "removed"],
      ["after", "added"],
    ] as const) {
      const row = rows.find((item) => item.kind === kind)!;
      const tokens = syntax[side].get(row)!;
      expect(textOf(tokens)).toBe(row.text);
      expect(tokens).toContainEqual({ text: "const", scope: "keyword" });
      expect(tokens.some((token) => token.scope === "number")).toBe(true);
      expect(tokens.some((token) => token.scope === "comment")).toBe(true);
    }
    expect(syntax.before.has(rows[0])).toBe(false);
    expect(syntax.after.has(rows[1])).toBe(false);
  });

  it("keeps both sides' multiline comment state independent, including context", () => {
    const rows = parsePatch(
      "@@ -1,3 +1,3 @@\n-/* open\n+// closed\n const value = 1;\n-*/\n+const next = 2;\n",
    );
    const context = rows.find((row) => row.kind === "context")!;
    const syntax = highlightPatch("x.ts", rows);
    expect(syntax.before.get(context)).toEqual([{ text: context.text, scope: "comment" }]);
    expect(syntax.after.get(context)).toContainEqual({ text: "const", scope: "keyword" });
  });

  it("resets lexer state across omitted hunk context", () => {
    const rows = parsePatch("@@ -1,1 +1,1 @@\n /* open\n@@ -100,1 +100,1 @@\n const next = 2;\n");
    const syntax = highlightPatch("x.ts", rows);
    for (const side of [syntax.before, syntax.after]) {
      expect(side.get(rows[1])).toEqual([{ text: "/* open", scope: "comment" }]);
      expect(side.get(rows[3])).toContainEqual({ text: "const", scope: "keyword" });
    }
  });

  it("preserves literal markup, Unicode, tabs and blank lines", () => {
    const rows = parsePatch(
      '@@ -1,3 +1,3 @@\n-const x = "😀 <script>old</script>";\n+const x = "😀 <script>new</script>";\n \t// 中文\n \n',
    );
    const syntax = highlightPatch("x.tsx", rows);
    for (const side of [syntax.before, syntax.after]) {
      for (const [row, tokens] of side) expect(textOf(tokens)).toBe(row.text);
    }
  });

  it("keeps unknown languages plain", () => {
    const rows = parsePatch("@@ -1,1 +1,1 @@\n const value = 1;\n");
    expect(highlightPatch("x.unknown", rows).after.get(rows[1])).toEqual([
      { text: rows[1].text, scope: null },
    ]);
  });

  it("refuses excessive character and row work before building token arrays", () => {
    const rows = parsePatch(`@@ -1,1 +1,1 @@\n+${"x".repeat(MAX_PATCH_SYNTAX_LENGTH + 1)}\n`);
    expect(highlightPatch("x.ts", rows).after.size).toBe(0);
    const many = parsePatch("@@ -1,1 +1,1 @@\n" + " const x = 1;\n".repeat(MAX_PATCH_SYNTAX_ROWS));
    expect(highlightPatch("x.ts", many).after.size).toBe(0);
  });

  it("caches tokens across visible-row reads and invalidates by path and patch", () => {
    let path = "x.ts";
    let rows = parsePatch("@@ -1,1 +1,1 @@\n const value = 1;\n");
    const read = cachedPatchSyntax(
      () => path,
      () => rows,
    );
    const first = read(rows[1]);
    expect(read(rows[1])).toBe(first);
    expect(first).toContainEqual({ text: "const", scope: "keyword" });
    path = "x.unknown";
    expect(read(rows[1])).not.toBe(first);
    expect(read(rows[1])).toEqual([{ text: rows[1].text, scope: null }]);
    rows = parsePatch("@@ -1,1 +1,1 @@\n const next = 2;\n");
    expect(textOf(read(rows[1]))).toBe("const next = 2;");
  });

  it("does not touch the patch until its first row is mounted", () => {
    const rows = parsePatch("@@ -1,1 +1,1 @@\n const value = 1;\n");
    let reads = 0;
    const read = cachedPatchSyntax(
      () => "x.ts",
      () => {
        reads += 1;
        return rows;
      },
    );
    expect(reads).toBe(0);
    read(rows[1]);
    expect(reads).toBe(1);
  });
});

describe("syntax with intra-line marks", () => {
  it("keeps a changed span contiguous across syntax tokens", () => {
    const tokens: SyntaxToken[] = [
      { text: "const", scope: "keyword" },
      { text: " value = ", scope: null },
      { text: '"new"', scope: "string" },
    ];
    const segments = syntaxSegments(tokens, { from: 2, to: 16 });
    expect(segments.map((segment) => segment.changed)).toEqual([false, true, false]);
    expect(textOf(segments[1].tokens)).toBe('nst value = "n');
    expect(segments[1].tokens.map((token) => token.scope)).toEqual(["keyword", null, "string"]);
    expect(segments.map((segment) => textOf(segment.tokens)).join("")).toBe(textOf(tokens));
  });

  it("combines a renamed literal's diff mark with its syntax colour", () => {
    const rows = parsePatch('@@ -1,1 +1,1 @@\n-const value = "old";\n+const value = "new";\n');
    const tokens = highlightPatch("x.ts", rows).after.get(rows[2])!;
    const span = intraLine(rows[1].text, rows[2].text)!.after;
    const changed = syntaxSegments(tokens, span).find((segment) => segment.changed)!;
    expect(textOf(changed.tokens)).toBe("new");
    expect(changed.tokens.every((token) => token.scope === "string")).toBe(true);
  });

  it("does not mark empty ranges or change unmarked token identity", () => {
    const tokens: SyntaxToken[] = [{ text: "const", scope: "keyword" }];
    expect(syntaxSegments(tokens)[0].tokens).toBe(tokens);
    expect(syntaxSegments(tokens, { from: 2, to: 2 })).toEqual([{ changed: false, tokens }]);
  });
});
