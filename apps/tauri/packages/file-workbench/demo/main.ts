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
    node("position").textContent = `${at.line}:${at.column}`;
  },
});
node("path").textContent = path;
node("changes").onclick = () => {
  if (dirty) {
    node("status").textContent = "Save this draft before switching files in the demo.";
    return;
  }
  path = "changes.ts";
  editor.setDoc(
    'const editor = "plain-text";\n\nexport const enabled = true;\n\nexport default editor;\n',
    false,
  );
  node("path").textContent = path;
  editor.setGrammar("typescript");
  editor.setGitChanges([
    {
      from: 1,
      to: 1,
      kind: "modified",
      rows: [
        {
          kind: "removed",
          before: 1,
          after: null,
          text: 'const editor = "CodeMirror";',
          emphasis: { from: 16, to: 26 },
        },
        {
          kind: "added",
          before: null,
          after: 1,
          text: 'const editor = "plain-text";',
          emphasis: { from: 16, to: 26 },
        },
      ],
    },
    {
      from: 3,
      to: 3,
      kind: "added",
      rows: [{ kind: "added", before: null, after: 3, text: "export const enabled = true;" }],
    },
    {
      from: 5,
      to: 5,
      kind: "deleted",
      rows: Array.from({ length: 240 }, (_, index) => ({
        kind: "removed",
        before: index + 4,
        after: null,
        text: `const removed${index} = ${index};`,
      })),
    },
  ]);
  node("status").textContent = "Click a gutter mark to inspect the change";
};
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
