// Regenerate `tests/fixtures/theme.json` from the palettes.
//
// The fixture is the colour contract `theme/tokens.test.ts` checks the app
// against. It used to be paired by hand with the Rust `theme tokens`;
// that crate went with the GUI, so the fixture now records what the
// palettes said at the moment a base was added or changed — which is still
// worth having, because a colour moving silently is the failure it catches.
// Run it deliberately, never as part of the build: a fixture regenerated
// automatically asserts nothing.
import { writeFileSync } from "node:fs";
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

const target = process.argv[2] ?? "tests/fixtures/theme.json";
writeFileSync(target, `${JSON.stringify(out, null, 2)}\n`);
console.log(`wrote ${target} (${out.bases.length} bases)`);
