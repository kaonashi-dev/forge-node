import { createFileExplorer, type FileEntry } from "@forge-node/file-workbench";
import "@forge-node/file-workbench/style.css";

function node(id: string): HTMLElement {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing demo element: ${id}`);
  return found;
}

const files: FileEntry[] = [
  { path: "README.md", kind: "File", ignored: false },
  { path: "src/main.ts", kind: "File", ignored: false },
  { path: "src/styles.css", kind: "File", ignored: false },
  { path: "docs/architecture.md", kind: "File", ignored: false },
];

const explorer = createFileExplorer(node("explorer"), {
  onOpen(path) {
    node("status").textContent = path;
  },
});
explorer.setState({ tree: { entries: files, truncated: false } });
node("status").textContent = "Pick a file.";
