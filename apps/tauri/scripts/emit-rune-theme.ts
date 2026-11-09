import { resolve } from "node:path";
import { RUNE_THEME_PATH, runeThemes } from "./rune-theme";

const out = resolve(import.meta.dir, "..", RUNE_THEME_PATH);
await Bun.write(out, runeThemes());
console.log(`wrote ${out}`);
