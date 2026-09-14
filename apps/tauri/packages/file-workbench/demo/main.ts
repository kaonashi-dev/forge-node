import { createFileExplorer, type FileEntry } from "@forge-node/file-workbench";
import { createFileEditor } from "@forge-node/file-workbench/editor";
import "@forge-node/file-workbench/style.css";

function node(id: string): HTMLElement {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing demo element: ${id}`);
  return found;
}
const files = new Map([
  [
    "README.md",
    "# File workbench\n\nA portable file surface.\n\nClick a file and start typing.\nUse the mouse, arrows, find, undo and your usual editing shortcuts.\n",
  ],
  ["src/main.ts", "export function greet(name: string): string {\n  return `Hello, ${name}`;\n}\n"],
  ["src/styles.css", ".workbench {\n  display: flex;\n  min-height: 0;\n}\n"],
  [
    "docs/architecture.md",
    "# Architecture\n\nThe host owns persistence. The package owns the view.\n",
  ],
]);
let path = "README.md";
let dirty = false;
const editor = createFileEditor(node("editor"), {
  doc: files.get(path)!,
  onChange() {
    dirty = true;
    node("status").textContent = "Unsaved";
  },
  onCursor(at) {
    node("position").textContent = `Ln ${at.line}, Col ${at.column}`;
  },
});
node("path").textContent = path;
const explorer = createFileExplorer(node("explorer"), {
  onOpen(next) {
    if (dirty) {
      node("status").textContent = "Save this draft before switching files in the demo.";
      return;
    }
    path = next;
    editor.setDoc(files.get(path) ?? "Generated fixture\n", false);
    node("path").textContent = path;
    node("status").textContent = "Saved";
  },
});
const entries: FileEntry[] = [...files.keys()].map((path) => ({
  path,
  kind: "File",
  ignored: false,
}));
explorer.setState({ tree: { entries, truncated: false } });
explorer.reveal(path);
node("save").onclick = () => {
  files.set(path, editor.text());
  dirty = false;
  node("status").textContent = "Saved";
};
node("change").onclick = () => {
  const next = `${files.get(path) ?? ""}\nExternal update\n`;
  files.set(path, next);
  if (dirty) {
    node("status").textContent = "Changed on disk · draft preserved";
    return;
  }
  editor.setDoc(next);
  node("status").textContent = "Updated from disk";
};
node("large").onclick = () => {
  explorer.setState({
    tree: {
      entries: Array.from({ length: 50_000 }, (_, i) => ({
        path: `fixture-${String(i).padStart(5, "0")}.ts`,
        kind: "File",
        ignored: false,
      })),
      truncated: false,
    },
  });
};
window.addEventListener("pagehide", () => {
  explorer.destroy();
  editor.destroy();
});
