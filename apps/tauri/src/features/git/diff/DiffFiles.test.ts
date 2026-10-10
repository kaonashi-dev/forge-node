import { describe, expect, it } from "vitest";

const sources = import.meta.glob("./DiffFiles.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const source = Object.values(sources)[0];
if (!source) throw new Error("DiffFiles source was not loaded");

const header = source.match(/<header\b[\s\S]*?<\/header>/)?.[0] ?? "";
const buttons = [...header.matchAll(/<button\b[\s\S]*?<\/button>/g)].map((match) => match[0]);

describe("diff file header actions", () => {
  it("keeps opening and folding on separate native buttons", () => {
    expect(buttons).toHaveLength(2);
    expect(buttons.every((button) => button.includes('type="button"'))).toBe(true);
    expect(header).not.toContain('role="button"');
  });

  it("opens the file at its first line through the existing Code callback", () => {
    const button = buttons.find((item) => item.includes("diff-file-name"));
    expect(button).toContain("props.onOpenLine(file.path, 1)");
    expect(button).toContain("Open ${file.path} in Code");
    expect(button).not.toContain("toggle(file.path)");
    expect(button).not.toContain("aria-expanded");
  });

  it("folds only from the arrow and exposes its expanded state", () => {
    const button = buttons.find((item) => item.includes("diff-file-toggle"));
    expect(button).toContain("toggle(file.path)");
    expect(button).toContain("aria-expanded={open()}");
    expect(button).not.toContain("onOpenLine");
  });
});
