import { describe, expect, it } from "vitest";
import { highlightExcerpt, MAX_SYNTAX_EXCERPT_LENGTH, syntaxHits } from "./searchSyntax";

const textOf = (tokens: { text: string }[]) => tokens.map((token) => token.text).join("");

describe("search syntax", () => {
  it.each([
    ["src/main.rs", 'let message = "hello"; // note', "let"],
    ["src/main.ts", 'const message: string = "hello"; // note', "const"],
    ["src/main.py", 'return "hello" # note', "return"],
  ])("colours %s without changing its text", (path, source, keyword) => {
    const [tokens] = highlightExcerpt(path, [source]);
    expect(textOf(tokens)).toBe(source);
    expect(tokens).toContainEqual({ text: keyword, scope: "keyword" });
    expect(tokens.some((token) => token.scope === "string" && token.text.includes("hello"))).toBe(
      true,
    );
    expect(tokens.some((token) => token.scope === "comment" && token.text.includes("note"))).toBe(
      true,
    );
  });

  it.each(["a.tsx", "a.jsx"])("preserves JSX tags and strings in %s", (path) => {
    const source = 'return <Panel title="Hello">{value}</Panel>;';
    const [tokens] = highlightExcerpt(path, [source]);
    expect(textOf(tokens)).toBe(source);
    expect(tokens.some((token) => token.scope === "tag")).toBe(true);
    expect(tokens.some((token) => token.scope === "string")).toBe(true);
  });

  it("keeps comment state across adjacent lines but not across excerpt gaps", () => {
    const lines = ["/* start", 'const value = "still a comment";', "*/ const value = 1;"];
    const rows = highlightExcerpt("a.ts", lines);
    expect(rows.map(textOf)).toEqual(lines);
    expect(rows[1]).toEqual([{ text: lines[1], scope: "comment" }]);
    expect(rows[2]).toContainEqual({ text: "const", scope: "keyword" });
    expect(highlightExcerpt("a.ts", ["const next = 1;"])[0]).toContainEqual({
      text: "const",
      scope: "keyword",
    });
  });

  it("preserves unicode, tabs, empty lines and literal HTML as text", () => {
    const lines = ['const emoji = "😀 café <script>alert(1)</script>";', "", "\t// 中文", ""];
    expect(highlightExcerpt("a.ts", lines).map(textOf)).toEqual(lines);
  });

  it("leaves unknown languages and oversized fragments plain", () => {
    const lines = ["const x = 1;", ""];
    expect(highlightExcerpt("a.unknown", lines)).toEqual(
      lines.map((text) => [{ text, scope: null }]),
    );
    const long = "x".repeat(MAX_SYNTAX_EXCERPT_LENGTH + 1);
    expect(highlightExcerpt("a.ts", [long])).toEqual([[{ text: long, scope: null }]]);
    expect(highlightExcerpt("a.ts", [])).toEqual([]);
  });

  it.each(["settings.json", "settings.yaml", "settings.toml", ".env.local"])(
    "colours configuration %s",
    (path) => {
      const source = path.endsWith("json")
        ? '{"message": "hello"}'
        : path.endsWith("yaml")
          ? 'message: "hello"'
          : 'message = "hello"';
      const [tokens] = highlightExcerpt(path, [source]);
      expect(textOf(tokens)).toBe(source);
      expect(tokens.some((token) => token.scope === "string")).toBe(true);
    },
  );
});

describe("syntax search marks", () => {
  it("keeps one complete mark when a match crosses syntax tokens", () => {
    const tokens = [
      { text: "const", scope: "keyword" as const },
      { text: " value = ", scope: null },
      { text: '"value"', scope: "string" as const },
    ];
    const segments = syntaxHits(tokens, "const value");
    expect(
      segments.filter((segment) => segment.hit).map((segment) => textOf(segment.tokens)),
    ).toEqual(["const value"]);
    expect(segments[0].tokens[0].scope).toBe("keyword");
    expect(segments.map((segment) => textOf(segment.tokens)).join("")).toBe(textOf(tokens));
  });

  it("marks all literal occurrences, including Unicode, without losing colours", () => {
    const [tokens] = highlightExcerpt("a.ts", ['const x = "😀.log .log"; // .log']);
    const segments = syntaxHits(tokens, ".log");
    expect(
      segments.filter((segment) => segment.hit).map((segment) => textOf(segment.tokens)),
    ).toEqual([".log", ".log", ".log"]);
    expect(
      segments.filter((segment) => segment.hit).map((segment) => segment.tokens[0].scope),
    ).toEqual(["string", "string", "comment"]);
    expect(syntaxHits(tokens, "").every((segment) => !segment.hit)).toBe(true);
  });
});
