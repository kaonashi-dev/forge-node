// Regenerate deliberately: doing so during a build would erase the regression baseline.
import { relative, resolve } from "node:path";
import { palettes, THEME_LABELS, type ThemeBaseId } from "../src/theme/tokens";

const out = {
  bases: Object.keys(palettes),
  palettes: Object.fromEntries(
    Object.entries(palettes).map(([id, palette]) => [
      id,
      { label: THEME_LABELS[id as ThemeBaseId], palette },
    ]),
  ),
};

const target = Bun.argv[2] ?? resolve(import.meta.dir, "../tests/fixtures/theme.json");
await Bun.write(target, `${JSON.stringify(out, null, 2)}\n`);
console.log(`wrote ${relative(process.cwd(), target)} (${out.bases.length} bases)`);
