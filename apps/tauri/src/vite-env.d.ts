/// <reference types="vite/client" />
/*
 * Vite's ambient types, for the two things source needs from the bundler
 * rather than from the DOM: `import.meta.env` and the asset query suffixes.
 * `?raw` is what lets `theme/stylesheets.test.ts` read a stylesheet as text
 * without `@types/node` — the suite runs under Vite, and reaching for
 * `node:fs` would type-check only by pulling Node's whole surface into an app
 * that has no business seeing it.
 */
