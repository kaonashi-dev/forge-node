import json
import sys

tree = json.load(open(sys.argv[1]))


def walk(node):
    yield node
    for child in node.get("children", []):
        yield from walk(child)


nodes = list(walk(tree["accessibility"]))
panes = [n for n in nodes if n.get("key") == "editor-pane"]
assert len(panes) == 1, f"expected one editor node, got {len(panes)}"
pane = panes[0]
assert pane.get("role") == "document", pane.get("role")
assert pane.get("name", "").startswith("Editor · "), pane.get("name")
assert "Line " in pane.get("description", ""), pane.get("description")
assert len(pane.get("value", "")) > 100, "the window's text is not readable"
inside = list(walk(pane))
assert not any(n.get("key", "").startswith(("row:", "text:", "gutter:")) for n in inside), (
    "rows leaked into the tree"
)
announce = [n for n in inside if n.get("key") == "editor-announce"]
assert len(announce) == 1 and announce[0].get("role") == "status", announce
print(
    f"ok: {len(nodes)} a11y nodes, editor subtree {len(inside)}, "
    f"value {len(pane['value'])} bytes, description {pane['description'][:60]!r}"
)
