import { describe, expect, it } from "vitest";
import {
  buildPathIndex,
  findPathRefs,
  refAt,
  resolvePath,
  splitPathRefs,
  type PathRef,
} from "./pathref";

const ROOT = "/Users/dev/forge-node";
const find = (text: string) => findPathRefs(text, ROOT);
const paths = (text: string) => find(text).map((ref) => ref.path);

describe("findPathRefs", () => {
  it("reads a path out of a sentence", () => {
    const refs = find("see apps/tauri/src/panels/GitPanel.tsx for the row.");
    expect(refs).toHaveLength(1);
    expect(refs[0]?.path).toBe("apps/tauri/src/panels/GitPanel.tsx");
    expect(refs[0]?.line).toBeNull();
  });

  it("covers the whole reference so a hover can underline it", () => {
    const text = "at src/main.rs:42:7 it panicked";
    const ref = find(text)[0] as PathRef;
    expect(text.slice(ref.from, ref.to)).toBe("src/main.rs:42:7");
    expect(ref).toMatchObject({ path: "src/main.rs", line: 42 });
  });

  it("takes the line from a compiler and from a review link", () => {
    expect(find("src/main.rs:42")[0]?.line).toBe(42);
    expect(find("src/main.rs#L42")[0]?.line).toBe(42);
  });

  it("drops the punctuation a clause leaves behind", () => {
    expect(paths("changed Cargo.toml, docs/ui.md.")).toEqual(["Cargo.toml", "docs/ui.md"]);
    expect(paths("in `src/lib.rs`")).toEqual(["src/lib.rs"]);
    expect(paths("(see docs/ui.md)")).toEqual(["docs/ui.md"]);
  });

  it("rewrites the three ways output spells the checkout", () => {
    expect(paths(`${ROOT}/apps/tauri/src/App.tsx`)).toEqual(["apps/tauri/src/App.tsx"]);
    expect(paths("~/dev/forge-node/apps/tauri/src/App.tsx")).toEqual(["apps/tauri/src/App.tsx"]);
    expect(paths("./scripts/dev.ts")).toEqual(["scripts/dev.ts"]);
    expect(paths("file:///Users/dev/forge-node/README.md")).toEqual(["README.md"]);
  });

  it("refuses a file outside the checkout, which cannot be opened", () => {
    expect(paths("/etc/hosts.conf")).toEqual([]);
    expect(paths("~/other/tree/App.tsx")).toEqual([]);
    expect(paths("../sibling/App.tsx")).toEqual([]);
  });

  it("is not fooled by the rest of a command line", () => {
    // Every one of these turned up in one screen of agent output.
    expect(paths("cargo check/clippy -p forge-tauri")).toEqual([]);
    expect(paths("tsc --noEmit is clean")).toEqual([]);
    expect(paths("pins Bun 1.4.1 and yours is 1.4.3-canary")).toEqual([]);
    expect(paths("e.g. i.e. etc.")).toEqual([]);
    expect(paths("bun add @codemirror/lang-css")).toEqual([]);
    expect(paths("https://example.com/a.html")).toEqual([]);
  });

  it("needs an extension only where the shape is a guess", () => {
    expect(paths("harness/progress")).toEqual([]);
    expect(paths("./harness/progress")).toEqual(["harness/progress"]);
    expect(paths("AGENTS.md")).toEqual(["AGENTS.md"]);
  });

  it("finds every reference on a line", () => {
    const refs = find("moved src/a.rs to src/b.rs");
    expect(refs.map((ref) => ref.path)).toEqual(["src/a.rs", "src/b.rs"]);
    expect(refAt(refs, 6)?.path).toBe("src/a.rs");
    expect(refAt(refs, 4)).toBeNull();
  });
});

describe("resolvePath", () => {
  const index = buildPathIndex([
    { path: "apps/tauri/src/App.tsx", kind: "File", ignored: false },
    { path: "crates/daemon/src/core.rs", kind: "File", ignored: false },
    { path: "crates/client/src/core.rs", kind: "File", ignored: false },
    { path: "docs", kind: "Directory", ignored: false },
    { path: "docs/ui.md", kind: "File", ignored: false },
  ]);

  it("passes the candidate through when nothing has read the checkout", () => {
    expect(resolvePath("anything.md", null)).toBe("anything.md");
  });

  it("answers an exact path and a diff spelling", () => {
    expect(resolvePath("docs/ui.md", index)).toBe("docs/ui.md");
    expect(resolvePath("b/docs/ui.md", index)).toBe("docs/ui.md");
  });

  it("completes a unique tail or basename", () => {
    expect(resolvePath("src/App.tsx", index)).toBe("apps/tauri/src/App.tsx");
    expect(resolvePath("App.tsx", index)).toBe("apps/tauri/src/App.tsx");
  });

  it("refuses to guess between two files with the same name", () => {
    expect(resolvePath("core.rs", index)).toBeNull();
    expect(resolvePath("src/core.rs", index)).toBeNull();
  });

  it("does not answer with a directory", () => {
    expect(resolvePath("docs", index)).toBeNull();
  });
});

describe("splitPathRefs", () => {
  it("keeps the line intact around the references", () => {
    const text = "moved src/a.rs to src/b.rs now";
    const pieces = splitPathRefs(text, findPathRefs(text, ROOT));
    expect(pieces.map((piece) => piece.text).join("")).toBe(text);
    expect(pieces.filter((piece) => piece.ref).map((piece) => piece.text)).toEqual([
      "src/a.rs",
      "src/b.rs",
    ]);
  });

  it("is one plain piece when there is nothing to link", () => {
    expect(splitPathRefs("nothing here", [])).toEqual([{ text: "nothing here", ref: null }]);
  });
});
