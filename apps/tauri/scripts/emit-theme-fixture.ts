// Regenerate deliberately: doing so during a build would erase the regression baseline.
import { relative, resolve } from "node:path";
import { palettes } from "../src/theme/tokens";

const LABELS: Record<string, string> = {
  "gruvbox-hard": "Gruvbox Dark Hard",
  gruvbox: "Gruvbox Dark",
  neutral: "Neutral Dark",
  "gruvbox-light": "Gruvbox Light",
  "neutral-light": "Neutral Light",
};

const out = {
  bases: Object.keys(palettes),
  palettes: Object.fromEntries(
    Object.entries(palettes).map(([id, palette]) => [id, { label: LABELS[id] ?? id, palette }]),
  ),
};

const target = Bun.argv[2] ?? resolve(import.meta.dir, "../tests/fixtures/theme.json");
await Bun.write(target, `${JSON.stringify(out, null, 2)}\n`);
console.log(`wrote ${relative(process.cwd(), target)} (${out.bases.length} bases)`);
