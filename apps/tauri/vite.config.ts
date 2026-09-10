import { defineConfig } from "vite";
import type { InlineConfig } from "vitest/node";
import solid from "vite-plugin-solid";
import { assertBun } from "./scripts/runtime";

assertBun();

export default defineConfig(({ mode }) => ({
  plugins: [solid()],
  clearScreen: false,
  // The bundle gate follows synchronous imports separately from deferred routes.
  build: { manifest: true },
  server: { port: 1420, strictPort: true },
  test: {
    include: ["src/**/*.test.{ts,tsx}", "tests/runtime.test.ts"],
    environment: "node",
    server: { deps: { inline: [/solid-js/] } },
  } satisfies InlineConfig,
  // Solid's server build does not exercise browser store reconciliation; production must keep its own conditions.
  ...(mode === "test" ? { resolve: { conditions: ["development", "browser"] } } : {}),
}));
