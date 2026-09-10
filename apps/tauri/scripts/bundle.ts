import type { Manifest } from "vite";

type Edge = "imports" | "dynamicImports";

/** Rollup's chunk names for the editor route's own modules. */
const EDITOR_CHUNKS = new Set(["EditorView", "createEditor"]);
const EDITOR_SOURCE = /(?:^|\/)src\/workbench\/(?:EditorView|editor\/createEditor)\.tsx?$/;

export function bundleGroups(manifest: Manifest): {
  initial: Set<string>;
  editor: Set<string>;
} {
  function reachable(start: string[], edges: Edge[], seen = new Set<string>()): Set<string> {
    for (const key of start) {
      if (seen.has(key)) continue;
      const chunk = manifest[key];
      if (!chunk) throw new Error(`Missing chunk in Vite manifest: ${key}`);
      seen.add(key);
      for (const edge of edges) reachable(chunk[edge] ?? [], edges, seen);
    }
    return seen;
  }

  const entry = Object.keys(manifest).filter((key) => manifest[key].isEntry);
  if (entry.length === 0) throw new Error("No entry in the Vite manifest — nothing to measure.");
  const initial = reachable(entry, ["imports"]);
  const all = reachable(entry, ["imports", "dynamicImports"]);
  if (initial.size >= all.size) {
    throw new Error("Every chunk is reachable synchronously — the route splits are gone.");
  }

  // Vite keys a chunk by its source path only while Rollup keeps a facade for it;
  // a merged dynamic entry falls back to `_<name>-<hash>.js`, so `name` is the
  // identity that survives both shapes — matching the path alone silently
  // measured a zero-byte editor route.
  const editorEntry = Object.keys(manifest).filter(
    (key) => EDITOR_SOURCE.test(key) || EDITOR_CHUNKS.has(manifest[key].name ?? ""),
  );
  if (editorEntry.length === 0) throw new Error("No editor route in the Vite manifest.");
  const editor = new Set(
    [...reachable(editorEntry, ["imports"])].filter((key) => !initial.has(key)),
  );
  if (editor.size === 0) throw new Error("The editor route is no longer deferred.");
  return { initial, editor };
}
