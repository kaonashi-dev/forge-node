import json
import sys

tree = json.load(open(sys.argv[1]))


def walk(node):
    yield node
    for child in node.get("children", []):
        yield from walk(child)


nodes = list(walk(tree["accessibility"]))
panes = [n for n in nodes if n.get("role") == "log"]
assert len(panes) == 1, f"expected exactly one terminal node, got {len(panes)}"
pane = panes[0]
assert pane.get("key") == "terminal-pane", pane.get("key")
assert "Cursor on row" in pane.get("description", ""), pane
assert len(pane.get("value", "")) > 100, "terminal text mirror is empty"
assert not pane.get("children"), "the terminal node must not expose per-cell children"
assert not any("cell" in n.get("key", "").lower() for n in nodes), "per-cell nodes leaked"
indicators = [n for n in nodes if n.get("key") == "terminal-focus-indicator"]
assert len(indicators) == 1, f"expected one focus indicator, got {len(indicators)}"
indicator = indicators[0]
assert indicator.get("role") == "status", indicator
assert "Keyboard input" in indicator.get("name", ""), indicator
assert "terminal" in indicator.get("description", "").lower(), indicator
print(
    f"ok: {len(nodes)} a11y nodes, 1 terminal node, 0 cell children, "
    f"focus indicator {indicator['name']!r}, mirror {len(pane['value'])} bytes"
)
