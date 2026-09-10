import { expect, test } from "bun:test";
import type { Manifest } from "vite";
import { bundleGroups } from "./bundle";

function fixture(): Manifest {
  return {
    "index.html": {
      file: "entry.js",
      isEntry: true,
      imports: ["shared"],
      dynamicImports: ["src/workbench/EditorView.tsx", "settings"],
    },
    shared: { file: "shared.js", imports: ["index.html"] },
    "src/workbench/EditorView.tsx": { file: "editor.js", imports: ["shared", "grammar"] },
    grammar: { file: "grammar.js" },
    settings: { file: "settings.js" },
  };
}

test("cycles terminate and shared chunks are charged only to the initial bundle", () => {
  const groups = bundleGroups(fixture());
  expect([...groups.initial]).toEqual(["index.html", "shared"]);
  expect([...groups.editor]).toEqual(["src/workbench/EditorView.tsx", "grammar"]);
});

test("a hashed chunk key is still found by its Rollup name", () => {
  const manifest = fixture();
  manifest["_EditorView-CHdbsWxN.js"] = {
    ...manifest["src/workbench/EditorView.tsx"],
    name: "EditorView",
  };
  delete manifest["src/workbench/EditorView.tsx"];
  manifest["index.html"].dynamicImports = ["_EditorView-CHdbsWxN.js", "settings"];
  expect([...bundleGroups(manifest).editor]).toEqual(["_EditorView-CHdbsWxN.js", "grammar"]);
});

test("missing manifest edges fail instead of undercounting", () => {
  const manifest = fixture();
  delete manifest.grammar;
  expect(() => bundleGroups(manifest)).toThrow("Missing chunk");
});

test("a missing editor entry cannot report a zero-byte editor", () => {
  const manifest = fixture();
  manifest["index.html"].dynamicImports = ["settings"];
  delete manifest["src/workbench/EditorView.tsx"];
  expect(() => bundleGroups(manifest)).toThrow("No editor route");
});

test("an eagerly imported editor fails even when another route remains lazy", () => {
  const manifest = fixture();
  manifest["index.html"].imports?.push("src/workbench/EditorView.tsx");
  expect(() => bundleGroups(manifest)).toThrow("no longer deferred");
});

test("a fully synchronous bundle fails the splitting contract", () => {
  const manifest = fixture();
  manifest["index.html"].imports?.push("src/workbench/EditorView.tsx", "settings");
  expect(() => bundleGroups(manifest)).toThrow("route splits are gone");
});

test("an empty manifest fails the measurement", () => {
  expect(() => bundleGroups({})).toThrow("No entry");
});
