import type { FileEntry, FileTree } from "../../../contracts/workbench";

const MAX_INDEX_ENTRIES = 50_000;
const MAX_INDEX_PATH_UNITS = 4 * 1024 * 1024;

/** Local directory observations add ignored paths without narrowing the global index. */
export function combineFileIndex(global: FileTree | null, local: FileTree | null): FileTree | null {
  if (!global && !local) return null;
  const workspace = global?.workspace_id ?? local!.workspace_id;
  const entries = new Map<string, FileEntry>();
  let units = 0;
  let truncated = !global || global.truncated || (local?.truncated ?? false);
  for (const tree of [local, global]) {
    if (!tree || tree.workspace_id !== workspace) continue;
    for (const entry of tree.entries) {
      if (entries.has(entry.path)) continue;
      if (entries.size >= MAX_INDEX_ENTRIES || units + entry.path.length > MAX_INDEX_PATH_UNITS) {
        truncated = true;
        break;
      }
      units += entry.path.length;
      entries.set(entry.path, entry);
    }
  }
  return { workspace_id: workspace, entries: [...entries.values()], truncated };
}
