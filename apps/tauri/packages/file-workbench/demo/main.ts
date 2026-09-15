import { createFileExplorer, type FileEntry } from "@forge-node/file-workbench";
import "@forge-node/file-workbench/style.css";

function node(id: string): HTMLElement {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing demo element: ${id}`);
  return found;
}

const files: FileEntry[] = [
  { path: "README.md", kind: "file" },
  { path: "src/main.ts", kind: "file" },
  { path: "src/styles.css", kind: "file" },
  { path: "docs/architecture.md", kind: "file" },
];

const explorer = createFileExplorer(node("explorer"), {
  onOpen(path) {
    node("status").textContent = path;
  },
});
explorer.setState({ tree: { root: "/demo", entries: files } });
node("status").textContent = "Pick a file.";
