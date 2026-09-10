// Budgets follow manifest edges so chunk renames cannot silently change the measurement.
import { join, resolve, relative, isAbsolute } from "node:path";
import type { Manifest } from "vite";
import { bundleGroups } from "./bundle";
import { readBytes, readJson } from "./files";

const dist = resolve(import.meta.dir, "../dist");
const manifest = (await readJson(join(dist, ".vite/manifest.json"), 1024 * 1024)) as Manifest;
const { initial, editor } = bundleGroups(manifest);
const MAX_CHUNK_BYTES = 16 * 1024 * 1024;
const sizes = new Map<string, number>();

for (const key of new Set([...initial, ...editor])) {
  const file = manifest[key].file;
  if (!file.endsWith(".js")) continue;
  const path = resolve(dist, file);
  const fromDist = relative(dist, path);
  if (fromDist.startsWith("..") || isAbsolute(fromDist)) {
    throw new Error(`Chunk outside dist: ${file}`);
  }
  if (!sizes.has(file)) {
    sizes.set(
      file,
      Bun.gzipSync(await readBytes(path, MAX_CHUNK_BYTES), { level: 6 }).length / 1024,
    );
  }
}

const sizeOf = (keys: Set<string>): number =>
  [...new Set([...keys].map((key) => manifest[key].file))].reduce(
    (sum, file) => sum + (sizes.get(file) ?? 0),
    0,
  );

const budgets = [
  { label: "initial JS", limit: 350, chunks: initial },
  { label: "editor route", limit: 250, chunks: editor },
];

let failed = false;
for (const budget of budgets) {
  const total = sizeOf(budget.chunks);
  const verdict = total <= budget.limit ? "ok" : "OVER";
  console.log(
    `${verdict.padEnd(4)} ${budget.label.padEnd(14)} ${total.toFixed(1)} KiB gz / ${budget.limit} KiB` +
      `  (${budget.chunks.size} chunks)`,
  );
  if (total > budget.limit) {
    failed = true;
    for (const key of [...budget.chunks].sort(
      (a, b) => (sizes.get(manifest[b].file) ?? 0) - (sizes.get(manifest[a].file) ?? 0),
    )) {
      console.log(
        `       ${manifest[key].file}  ${(sizes.get(manifest[key].file) ?? 0).toFixed(1)} KiB gz`,
      );
    }
  }
}

if (failed) {
  console.error("\nBundle over budget (docs/performance.md).");
  process.exit(1);
}
