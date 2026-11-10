#!/usr/bin/env bun
import { check, length, loadTree, prefix, repr, walk } from "./a11y-tree.ts";

const path = process.argv[2];
if (path === undefined) {
  console.error("usage: check_editor_tree.ts <tree.json>");
  process.exit(2);
}

const nodes = walk(loadTree(path));
const panes = nodes.filter((n) => n.key === "editor-pane");
check(panes.length === 1, `expected one editor node, got ${panes.length}`);
const pane = panes[0]!;
check(pane.role === "document", pane.role);
check((pane.name ?? "").startsWith("Editor · "), pane.name);
check((pane.description ?? "").includes("Line "), pane.description);
check(length(pane.value ?? "") > 100, "the window's text is not readable");
const inside = walk(pane);
const ROW_KEYS = ["row:", "text:", "gutter:"];
check(
  !inside.some((n) => ROW_KEYS.some((p) => (n.key ?? "").startsWith(p))),
  "rows leaked into the tree",
);
const announce = inside.filter((n) => n.key === "editor-announce");
check(announce.length === 1 && announce[0]!.role === "status", announce);
console.log(
  `ok: ${nodes.length} a11y nodes, editor subtree ${inside.length}, ` +
    `value ${length(pane.value ?? "")} bytes, description ${repr(prefix(pane.description ?? "", 60))}`,
);
