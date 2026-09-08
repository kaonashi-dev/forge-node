// P11: the bundle budgets from `plan-ui-ux.md` §3.3, enforced on `dist/`.
//
// Not a vitest: the numbers only exist after a real Rollup build, and a suite
// that had to run one would take the unit tests from six seconds to a minute.
// `pnpm build` runs it, so the budget is checked wherever the bundle is made —
// including CI, which builds.
//
// The set of chunks is read from Vite's manifest rather than matched by file
// name. `imports` is what a chunk pulls in synchronously and `dynamicImports`
// is what it defers, so "what does the first frame cost" is a graph walk and
// not a guess — and it keeps being right when Rollup renames a chunk, which a
// name pattern does not.

import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const DIST = new URL("../dist/", import.meta.url).pathname;
const manifest = JSON.parse(readFileSync(join(DIST, ".vite/manifest.json"), "utf8"));

/** Every chunk reachable from `start`, following the edges named in `keys`. */
function reachable(start, keys, seen = new Set()) {
  for (const key of start) {
    if (seen.has(key)) continue;
    const chunk = manifest[key];
    if (!chunk) continue;
    seen.add(key);
    for (const edge of keys) reachable(chunk[edge] ?? [], keys, seen);
  }
  return seen;
}

const entry = Object.keys(manifest).filter((key) => manifest[key].isEntry);
if (entry.length === 0) {
  console.error("No entry in the Vite manifest — nothing to measure.");
  process.exit(1);
}

/** Synchronous from the entry: what the window must have before it paints. */
const initial = reachable(entry, ["imports"]);
/** Everything, so a route's cost is what it adds on top of the initial set. */
const all = reachable(entry, ["imports", "dynamicImports"]);

/** The editor route: its own module and everything it alone drags in. */
const editorEntry = Object.keys(manifest).filter((key) =>
  /src\/workbench\/(EditorView|editor\/createEditor)\.tsx?$/.test(key),
);
const editor = new Set([...reachable(editorEntry, ["imports"])].filter((key) => !initial.has(key)));

const sizeOf = (keys) =>
  [...keys]
    .map((key) => manifest[key].file)
    .filter((file) => file.endsWith(".js"))
    .reduce((sum, file) => sum + gzipSync(readFileSync(join(DIST, file))).length / 1024, 0);

const BUDGETS = [
  { label: "initial JS", limit: 350, chunks: initial },
  { label: "editor route", limit: 250, chunks: editor },
];

let failed = false;
for (const budget of BUDGETS) {
  const total = sizeOf(budget.chunks);
  const verdict = total <= budget.limit ? "ok" : "OVER";
  console.log(
    `${verdict.padEnd(4)} ${budget.label.padEnd(14)} ${total.toFixed(1)} kB gz / ${budget.limit} kB` +
      `  (${budget.chunks.size} chunks)`,
  );
  if (total > budget.limit) {
    failed = true;
    for (const key of [...budget.chunks].sort((a, b) => sizeOf([b]) - sizeOf([a]))) {
      console.log(`       ${manifest[key].file}  ${sizeOf([key]).toFixed(1)} kB gz`);
    }
  }
}

// A sanity check on the measurement itself: if the "initial" set is the whole
// bundle, the lazy routes have stopped being lazy and the budget above is
// measuring nothing useful.
if (initial.size >= all.size) {
  console.error("\nEvery chunk is reachable synchronously — the route splits are gone.");
  process.exit(1);
}

if (failed) {
  console.error("\nBundle over budget (plan-ui-ux.md §3.3).");
  process.exit(1);
}
