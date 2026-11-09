import { expect, test } from "bun:test";
import { resolve } from "node:path";
import { RUNE_THEME_PATH, runeThemes } from "./rune-theme";

test("the Rune client's theme file matches tokens.ts", async () => {
  const committed = await Bun.file(resolve(import.meta.dir, "..", RUNE_THEME_PATH)).text();
  // A failure here means a token changed without `bun run theme:rune`.
  expect(committed).toBe(runeThemes());
});
