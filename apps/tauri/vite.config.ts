import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

/**
 * `mode` is `"test"` under Vitest and `"development"` / `"production"` under
 * the dev server and the build.
 *
 * The condition override below is scoped to the test run on purpose. Vitest
 * transforms in SSR mode, so without it the suite resolves solid-js's *server*
 * build — where a store is a plain object and `reconcile` is a shallow
 * key-by-key copy with no diffing at all. Nothing tested that before, so
 * nothing noticed; the moment `forgeStore` started reconciling, the suite was
 * measuring a different implementation from the one the WebView runs.
 *
 * Applying it unconditionally would be the opposite mistake: `development`
 * would pull solid's dev build into the shipped bundle.
 */
export default defineConfig(({ mode }) => ({
  plugins: [solid()],
  clearScreen: false,
  /*
   * The manifest is what `scripts/check-bundle.mjs` reads to know which chunks
   * the entry pulls in *synchronously* — the ones a window has to have before
   * it paints — as opposed to the ones behind a `lazy()`. Matching on chunk
   * file names was the first cut and it silently stopped measuring anything
   * the moment Rollup renamed a chunk (§3.2 P11).
   */
  build: { manifest: true },
  server: {
    port: 1420,
    strictPort: true,
  },
  test: {
    environment: "node",
    server: { deps: { inline: [/solid-js/] } },
  },
  ...(mode === "test" ? { resolve: { conditions: ["development", "browser"] } } : {}),
}));
