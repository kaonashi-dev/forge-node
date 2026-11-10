#!/usr/bin/env bun
import { check, length, loadTree, repr, walk } from "./a11y-tree.ts";

const path = process.argv[2];
if (path === undefined) {
  console.error("usage: check_tree.ts <tree.json>");
  process.exit(2);
}

const nodes = walk(loadTree(path));
const panes = nodes.filter((n) => n.role === "log");
check(panes.length === 1, `expected exactly one terminal node, got ${panes.length}`);
const pane = panes[0]!;
check(pane.key === "terminal-pane", pane.key);
check((pane.description ?? "").includes("Cursor on row"), pane);
check(length(pane.value ?? "") > 100, "terminal text mirror is empty");
check(!pane.children?.length, "the terminal node must not expose per-cell children");
check(
  !nodes.some((n) => (n.key ?? "").toLowerCase().includes("cell")),
  "per-cell nodes leaked",
);
const indicators = nodes.filter((n) => n.key === "terminal-focus-indicator");
check(indicators.length === 1, `expected one focus indicator, got ${indicators.length}`);
const indicator = indicators[0]!;
check(indicator.role === "status", indicator);
check((indicator.name ?? "").includes("Keyboard input"), indicator);
check((indicator.description ?? "").toLowerCase().includes("terminal"), indicator);
console.log(
  `ok: ${nodes.length} a11y nodes, 1 terminal node, 0 cell children, ` +
    `focus indicator ${repr(indicator.name)}, mirror ${length(pane.value ?? "")} bytes`,
);
