import { describe, expect, it } from "vitest";
import { highlight, tokensToHtml } from "../../../packages/file-workbench/src/highlight";

describe("highlight", () => {
  it("colours TypeScript keywords, strings and comments", () => {
    const tokens = highlight(
      '// note\nimport { x } from "mod";\nexport type Id = string;\n',
      "typescript",
    );
    const scopes = tokens.filter((t) => t.scope !== "plain").map((t) => [t.text, t.scope]);
    expect(scopes).toEqual(
      expect.arrayContaining([
        ["// note", "comment"],
        ["import", "keyword"],
        ["from", "keyword"],
        ['"mod"', "string"],
        ["export", "keyword"],
        ["type", "keyword"],
        ["Id", "type"],
        ["string", "type"],
      ]),
    );
  });

  it("leaves an unknown grammar plain", () => {
    expect(highlight("const x = 1", null)).toEqual([{ text: "const x = 1", scope: "plain" }]);
  });

  it("escapes HTML in the overlay", () => {
    expect(tokensToHtml([{ text: "<script>", scope: "plain" }])).toContain("&lt;script&gt;");
  });

  it("resumes highlighting after a closed template literal", () => {
    const source = "const a = `hello ${name} world`;\nconst b = 1;\n";
    const tokens = highlight(source, "javascript");
    const joined = tokens.map((t) => t.text).join("");
    expect(joined).toBe(source);

    // Everything after the template must not be swallowed as a string.
    const after = tokens.reduce(
      (acc, t) => {
        acc.text += t.text;
        if (acc.text.includes("world`;") && t.text.includes("const")) acc.sawConst = t.scope;
        if (acc.text.includes("= 1") && t.scope === "number") acc.sawNumber = true;
        return acc;
      },
      { text: "", sawConst: "" as string, sawNumber: false },
    );
    expect(after.sawConst).toBe("keyword");
    expect(after.sawNumber).toBe(true);

    const stringChunks = tokens.filter((t) => t.scope === "string").map((t) => t.text);
    expect(stringChunks.some((s) => s.includes("hello"))).toBe(true);
    expect(stringChunks.some((s) => s.includes("world"))).toBe(true);
    expect(stringChunks.join("")).not.toContain("const b");
  });

  it("handles nested template literals inside interpolations", () => {
    const source = "const x = `a ${`b ${c}`} d`;\nlet y = true;\n";
    const tokens = highlight(source, "javascript");
    expect(tokens.map((t) => t.text).join("")).toBe(source);
    expect(tokens.some((t) => t.text === "let" && t.scope === "keyword")).toBe(true);
    expect(tokens.some((t) => t.text === "true" && t.scope === "constant")).toBe(true);
  });
});
