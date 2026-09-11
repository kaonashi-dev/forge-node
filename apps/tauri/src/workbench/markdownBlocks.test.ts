import { describe, expect, it } from "vitest";
import { inlineSpans, parseMarkdown, safeHref } from "./markdownBlocks";

const BODY = [
  "## Summary",
  "- Adds the `NEQUI` rail and **WOMPI** vendor",
  "  including the fee segments",
  "",
  "## Test plan",
  "- [ ] Run the migration",
  "- [x] `npm run check`",
].join("\n");

describe("parseMarkdown", () => {
  it("reads a pull request body as headings, lists and tasks", () => {
    const blocks = parseMarkdown(BODY);
    expect(blocks.map((block) => block.kind)).toEqual(["heading", "list", "heading", "list"]);

    const summary = blocks[1];
    if (summary.kind !== "list") throw new Error("expected a list");
    expect(summary.ordered).toBe(false);
    // The un-marked second line continues the bullet rather than opening a
    // paragraph of its own.
    expect(summary.items).toHaveLength(1);
    expect(summary.items[0].checked).toBeNull();
    expect(summary.items[0].spans).toEqual([
      { kind: "text", text: "Adds the ", strong: false, em: false },
      { kind: "code", text: "NEQUI" },
      { kind: "text", text: " rail and ", strong: false, em: false },
      { kind: "text", text: "WOMPI", strong: true, em: false },
      { kind: "text", text: " vendor\nincluding the fee segments", strong: false, em: false },
    ]);

    const plan = blocks[3];
    if (plan.kind !== "list") throw new Error("expected a list");
    expect(plan.items.map((item) => item.checked)).toEqual([false, true]);
  });

  it("keeps a fenced block whole, including the lines that look like markup", () => {
    const blocks = parseMarkdown("```sh\n# not a heading\n- not a bullet\n```\nafter");
    expect(blocks[0]).toEqual({
      kind: "code",
      lang: "sh",
      text: "# not a heading\n- not a bullet",
    });
    expect(blocks[1].kind).toBe("paragraph");
  });

  it("closes an unterminated fence at the end of the body", () => {
    const blocks = parseMarkdown("```\nstill open");
    expect(blocks).toEqual([{ kind: "code", lang: null, text: "still open" }]);
  });

  it("nests by indent, up to the cap", () => {
    const blocks = parseMarkdown("- one\n  - two\n        - eight");
    if (blocks[0].kind !== "list") throw new Error("expected a list");
    expect(blocks[0].items.map((item) => item.depth)).toEqual([0, 1, 2]);
  });

  it("starts a second list when the marker changes kind", () => {
    const blocks = parseMarkdown("1. first\n- bullet");
    expect(blocks.map((block) => block.kind === "list" && block.ordered)).toEqual([true, false]);
  });

  it("reads a table only when the delimiter row is there", () => {
    const table = parseMarkdown("| a | b |\n| --- | :-: |\n| 1 | 2 |");
    expect(table[0]).toEqual({
      kind: "table",
      head: [
        [{ kind: "text", text: "a", strong: false, em: false }],
        [{ kind: "text", text: "b", strong: false, em: false }],
      ],
      rows: [
        [
          [{ kind: "text", text: "1", strong: false, em: false }],
          [{ kind: "text", text: "2", strong: false, em: false }],
        ],
      ],
    });
    expect(parseMarkdown("a | b").map((block) => block.kind)).toEqual(["paragraph"]);
  });

  it("separates a rule from a bullet", () => {
    expect(parseMarkdown("---").map((block) => block.kind)).toEqual(["rule"]);
    expect(parseMarkdown("- one").map((block) => block.kind)).toEqual(["list"]);
  });

  it("parses a quote as blocks of its own", () => {
    const blocks = parseMarkdown("> ## quoted\n> - item\n\nout");
    if (blocks[0].kind !== "quote") throw new Error("expected a quote");
    expect(blocks[0].blocks.map((block) => block.kind)).toEqual(["heading", "list"]);
  });

  it("drops the tags a README centres an image with, and the gap they leave", () => {
    const blocks = parseMarkdown('<p align="center">\n  <img src="logo.png" width="120">\n</p>');
    expect(blocks).toEqual([
      {
        kind: "paragraph",
        spans: [{ kind: "image", alt: "", src: "logo.png", href: null, width: "120px" }],
      },
    ]);
  });

  it("hides comments, on one line or across several", () => {
    const blocks = parseMarkdown(
      "<!-- a note -->\nkept <!-- inline --> here\n\n<!--\nhidden\n-->\nafter",
    );
    expect(blocks).toEqual([
      {
        kind: "paragraph",
        spans: [{ kind: "text", text: "kept  here", strong: false, em: false }],
      },
      { kind: "paragraph", spans: [{ kind: "text", text: "after", strong: false, em: false }] },
    ]);
  });
});

describe("inlineSpans", () => {
  it("leaves an underscored identifier alone", () => {
    expect(inlineSpans("PAYIN_CO_NEQUI and PAYOUT_CO_NEQUI")).toEqual([
      { kind: "text", text: "PAYIN_CO_NEQUI and PAYOUT_CO_NEQUI", strong: false, em: false },
    ]);
    expect(inlineSpans("_stressed_")).toEqual([
      { kind: "text", text: "stressed", strong: false, em: true },
    ]);
  });

  it("does not open emphasis on a dangling marker", () => {
    expect(inlineSpans("2 * 3 * 4")).toEqual([
      { kind: "text", text: "2 * 3 * 4", strong: false, em: false },
    ]);
  });

  it("keeps markup inside a code span as text", () => {
    expect(inlineSpans("`**not bold**`")).toEqual([{ kind: "code", text: "**not bold**" }]);
  });

  it("links what a browser can open, and only that", () => {
    expect(inlineSpans("[docs](https://example.com/a)")).toEqual([
      { kind: "link", text: "docs", href: "https://example.com/a" },
    ]);
    expect(inlineSpans("see https://example.com/a.")).toEqual([
      { kind: "text", text: "see ", strong: false, em: false },
      { kind: "link", text: "https://example.com/a", href: "https://example.com/a" },
      { kind: "text", text: ".", strong: false, em: false },
    ]);
    expect(inlineSpans("[x](javascript:alert(1))")).toEqual([
      { kind: "text", text: "[x](javascript:alert(1))", strong: false, em: false },
    ]);
  });

  it("reads an image with its source unresolved", () => {
    expect(inlineSpans('see ![a chart](./chart.png "title")')).toEqual([
      { kind: "text", text: "see ", strong: false, em: false },
      { kind: "image", alt: "a chart", src: "./chart.png", href: null, width: null },
    ]);
    expect(inlineSpans("![spaced](<my shot.png>)")).toEqual([
      { kind: "image", alt: "spaced", src: "my shot.png", href: null, width: null },
    ]);
  });

  it("keeps a badge's image whole inside its link", () => {
    expect(inlineSpans("[![CI](https://example.com/ci.svg)](https://example.com/actions)")).toEqual(
      [
        {
          kind: "image",
          alt: "CI",
          src: "https://example.com/ci.svg",
          href: "https://example.com/actions",
          width: null,
        },
      ],
    );
  });

  it("reads an img tag's source and alt, and a br as a break", () => {
    expect(inlineSpans(`one<br>two <img alt='the "logo"' src=a.svg width="50%" />`)).toEqual([
      { kind: "text", text: "one\ntwo ", strong: false, em: false },
      { kind: "image", alt: 'the "logo"', src: "a.svg", href: null, width: "50%" },
    ]);
  });

  it("leaves a tag it does not know as text", () => {
    expect(inlineSpans("<script>x</script>")).toEqual([
      { kind: "text", text: "<script>x</script>", strong: false, em: false },
    ]);
  });
});

describe("safeHref", () => {
  it("passes http, https and mailto, and refuses the rest", () => {
    expect(safeHref("https://example.com")).toBe("https://example.com");
    expect(safeHref("mailto:someone@example.com")).toBe("mailto:someone@example.com");
    expect(safeHref(" JavaScript:alert(1)")).toBeNull();
    expect(safeHref("./docs/readme.md")).toBeNull();
  });
});
