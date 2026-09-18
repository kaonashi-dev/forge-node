import { expect, test } from "bun:test";
import type { Manifest } from "vite";
import { bundleGroups } from "./bundle";

const CELLS = "src/features/editor/cells/EditorTerminalPane.tsx";
const DOM = "src/features/editor/dom/EditorView.tsx";

function fixture(): Manifest {
  return {
    "index.html": {
      file: "entry.js",
      isEntry: true,
      imports: ["shared"],
      dynamicImports: [CELLS, DOM, "settings"],
    },
    shared: { file: "shared.js", imports: ["index.html"] },
    [CELLS]: { file: "editor.js", imports: ["shared", "grammar"] },
    [DOM]: { file: "editor-dom.js", imports: ["shared"] },
    grammar: { file: "grammar.js" },
    settings: { file: "settings.js" },
  };
}

test("cycles terminate and shared chunks are charged only to the initial bundle", () => {
  const groups = bundleGroups(fixture());
  expect([...groups.initial]).toEqual(["index.html", "shared"]);
  expect([...groups.editor]).toEqual([CELLS, "grammar", DOM]);
});

test("a hashed chunk key is still found by its Rollup name", () => {
  const manifest = fixture();
  manifest["_EditorTerminalPane-CHdbsWxN.js"] = {
    ...manifest[CELLS],
    name: "EditorTerminalPane",
  };
  delete manifest[CELLS];
  manifest["index.html"].dynamicImports = ["_EditorTerminalPane-CHdbsWxN.js", DOM, "settings"];
  expect([...bundleGroups(manifest).editor]).toEqual([
    DOM,
    "_EditorTerminalPane-CHdbsWxN.js",
    "grammar",
  ]);
});

test("missing manifest edges fail instead of undercounting", () => {
  const manifest = fixture();
  delete manifest.grammar;
  expect(() => bundleGroups(manifest)).toThrow("Missing chunk");
});

test("a missing editor entry cannot report a zero-byte editor", () => {
  const manifest = fixture();
  manifest["index.html"].dynamicImports = ["settings"];
  delete manifest[CELLS];
  delete manifest[DOM];
  expect(() => bundleGroups(manifest)).toThrow("No editor route");
});

test("an eagerly imported editor fails even when another route remains lazy", () => {
  const manifest = fixture();
  manifest["index.html"].imports?.push(CELLS);
  expect(() => bundleGroups(manifest)).toThrow("no longer deferred");
});

test("a fully synchronous bundle fails the splitting contract", () => {
  const manifest = fixture();
  manifest["index.html"].imports?.push(CELLS, DOM, "settings");
  expect(() => bundleGroups(manifest)).toThrow("route splits are gone");
});

test("an empty manifest fails the measurement", () => {
  expect(() => bundleGroups({})).toThrow("No entry");
});
