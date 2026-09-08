// Which language mark a file wears, by name alone.
//
// By name and never by content: the tree is painted from `ListFiles`, which is
// a list of names, and reading a file to find out what it is would cost a round
// trip per row. A name that says nothing gets the plain sheet rather than a
// guess, so a wrong mark is never asserted.
//
// The upstream icon theme ships a suffix table of 66 exotic entries (`jspx`,
// `wsdl`, `karma.conf.js`) and leaves `rs`, `ts`, `py`, `json` to the editor's
// own defaults, which are not in that repository. This table is therefore ours
// to write and ours to keep true; `langIcons.test.ts` is what holds it to the
// files that were actually vendored.

/**
 * Every vendored mark.
 *
 * The list is the type: a name that has no SVG under `public/icons/lang` fails
 * the test rather than rendering an empty box, and a file nothing names fails
 * it too — the set on disk and the set in code are one thing.
 */
export const LANG_ICON_NAMES = [
  "actionscript",
  "angular",
  "api",
  "archive",
  "astro",
  "c",
  "coffeescript",
  "cpp",
  "css",
  "csv",
  "cucumber",
  "dart",
  "default",
  "deno",
  "diagram",
  "docker",
  "docker_compose",
  "editorconfig",
  "elixir",
  "env",
  "eslint",
  "font",
  "gleam",
  "go",
  "graphql",
  "groovy",
  "h",
  "handlebars",
  "haskell",
  "hcl",
  "html",
  "idl",
  "ignored",
  "image",
  "java",
  "java_class",
  "javascript",
  "jest",
  "js_test",
  "jshint",
  "json",
  "jsp",
  "jspx",
  "jsx",
  "jsx_test",
  "karma",
  "kotlin",
  "m",
  "makefile",
  "manifest",
  "markdown",
  "nextjs",
  "nix",
  "npm",
  "nuxt",
  "open_api",
  "patch",
  "perl",
  "php",
  "php_unit",
  "prisma",
  "python",
  "redis",
  "ruby",
  "rust",
  "sass",
  "scala",
  "settings",
  "sql",
  "storage",
  "stylelint",
  "swift",
  "terminal",
  "terraform",
  "toml",
  "ts_test",
  "tsx",
  "tsx_test",
  "typescript",
  "vcs",
  "vue",
  "wsdl",
  "xhtml",
  "xml",
  "xsd",
  "yaml",
  "yarn",
] as const;

export type LangIconName = (typeof LANG_ICON_NAMES)[number];

/** The mark for a file with nothing to go on. */
export const DEFAULT_LANG_ICON: LangIconName = "default";

/** A whole file name, lowercased. Beats every rule below it. */
const BY_NAME: Record<string, LangIconName> = {
  dockerfile: "docker",
  "docker-compose.yml": "docker_compose",
  "docker-compose.yaml": "docker_compose",
  "compose.yml": "docker_compose",
  "compose.yaml": "docker_compose",
  makefile: "makefile",
  gnumakefile: "makefile",
  "cargo.toml": "rust",
  "cargo.lock": "rust",
  "package.json": "npm",
  "package-lock.json": "npm",
  "pnpm-lock.yaml": "npm",
  "pnpm-workspace.yaml": "npm",
  "yarn.lock": "yarn",
  ".npmrc": "npm",
  ".nvmrc": "npm",
  "tsconfig.json": "typescript",
  "jsconfig.json": "javascript",
  "angular.json": "angular",
  "deno.json": "deno",
  "deno.jsonc": "deno",
  "deno.lock": "deno",
  "phpunit.xml": "php_unit",
  "composer.json": "php",
  gemfile: "ruby",
  "gemfile.lock": "ruby",
  rakefile: "ruby",
  "go.mod": "go",
  "go.sum": "go",
  "build.gradle": "groovy",
  "settings.gradle": "groovy",
  "pom.xml": "java",
  "manifest.mf": "manifest",
  ".editorconfig": "editorconfig",
  ".jshintrc": "jshint",
  ".gitattributes": "vcs",
  ".gitmodules": "vcs",
  ".gitkeep": "vcs",
  ".mailmap": "vcs",
  "openapi.json": "open_api",
  "openapi.yaml": "open_api",
  "swagger.json": "open_api",
};

/**
 * A name that starts with one of these takes its mark.
 *
 * Config files carry a version, a stage or a module system in the middle
 * (`.env.local`, `jest.config.mjs`, `Dockerfile.ci`), so the extension is the
 * wrong end of the name to read.
 */
const BY_PREFIX: ReadonlyArray<readonly [string, LangIconName]> = [
  [".env", "env"],
  [".eslintrc", "eslint"],
  ["eslint.config.", "eslint"],
  [".stylelintrc", "stylelint"],
  ["stylelint.config.", "stylelint"],
  ["jest.config.", "jest"],
  ["karma.conf.", "karma"],
  ["next.config.", "nextjs"],
  ["nuxt.config.", "nuxt"],
  ["dockerfile", "docker"],
  ["docker-compose.", "docker_compose"],
];

/** A name ending in one of these takes its mark, extension table skipped. */
const BY_SUFFIX: ReadonlyArray<readonly [string, LangIconName]> = [
  [".test.ts", "ts_test"],
  [".spec.ts", "ts_test"],
  [".test.mts", "ts_test"],
  [".test.tsx", "tsx_test"],
  [".spec.tsx", "tsx_test"],
  [".test.js", "js_test"],
  [".spec.js", "js_test"],
  [".test.mjs", "js_test"],
  [".test.cjs", "js_test"],
  [".test.jsx", "jsx_test"],
  [".spec.jsx", "jsx_test"],
  [".d.ts", "typescript"],
  ["ignore", "ignored"],
];

const BY_EXTENSION: Record<string, LangIconName> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  as: "actionscript",
  coffee: "coffeescript",
  vue: "vue",
  astro: "astro",
  rs: "rust",
  go: "go",
  py: "python",
  pyi: "python",
  pyw: "python",
  rb: "ruby",
  gemspec: "ruby",
  erb: "ruby",
  java: "java",
  class: "java_class",
  jar: "archive",
  kt: "kotlin",
  kts: "kotlin",
  scala: "scala",
  sc: "scala",
  groovy: "groovy",
  gradle: "groovy",
  swift: "swift",
  c: "c",
  h: "h",
  hh: "h",
  cc: "cpp",
  cpp: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  hxx: "cpp",
  m: "m",
  mm: "m",
  php: "php",
  pl: "perl",
  pm: "perl",
  ex: "elixir",
  exs: "elixir",
  hs: "haskell",
  gleam: "gleam",
  dart: "dart",
  nix: "nix",
  tf: "terraform",
  tfvars: "terraform",
  hcl: "hcl",
  sh: "terminal",
  bash: "terminal",
  zsh: "terminal",
  fish: "terminal",
  mk: "makefile",
  sql: "sql",
  prisma: "prisma",
  graphql: "graphql",
  gql: "graphql",
  html: "html",
  htm: "html",
  xhtml: "xhtml",
  xml: "xml",
  xsd: "xsd",
  wsdl: "wsdl",
  idl: "idl",
  jsp: "jsp",
  jspx: "jspx",
  uml: "diagram",
  css: "css",
  scss: "sass",
  sass: "sass",
  json: "json",
  jsonc: "json",
  jsonl: "json",
  json5: "json",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  properties: "settings",
  ini: "settings",
  cfg: "settings",
  conf: "settings",
  md: "markdown",
  mdx: "markdown",
  markdown: "markdown",
  csv: "csv",
  tsv: "csv",
  feature: "cucumber",
  http: "api",
  rest: "api",
  patch: "patch",
  diff: "patch",
  hbs: "handlebars",
  handlebars: "handlebars",
  mustache: "handlebars",
  svg: "image",
  png: "image",
  jpg: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  bmp: "image",
  ico: "image",
  icns: "image",
  ttf: "font",
  otf: "font",
  woff: "font",
  woff2: "font",
  zip: "archive",
  tar: "archive",
  gz: "archive",
  tgz: "archive",
  bz2: "archive",
  xz: "archive",
  rar: "archive",
  "7z": "archive",
  db: "storage",
  sqlite: "storage",
  sqlite3: "storage",
  rdb: "redis",
};

/** The last path segment, lowercased. Callers pass a name or a whole path. */
function fileName(path: string): string {
  const cut = path.lastIndexOf("/");
  return (cut === -1 ? path : path.slice(cut + 1)).toLowerCase();
}

/**
 * The mark for one file.
 *
 * Whole name first, then the prefixes and the compound suffixes, then the
 * extension: `next.config.js` is Next and not JavaScript, and `app.test.ts` is
 * a test and not TypeScript, and only the longer rule can say so.
 */
export function langIconFor(path: string): LangIconName {
  const name = fileName(path);
  const exact = BY_NAME[name];
  if (exact) return exact;

  for (const [prefix, icon] of BY_PREFIX) {
    if (name.startsWith(prefix)) return icon;
  }
  for (const [suffix, icon] of BY_SUFFIX) {
    if (name.endsWith(suffix)) return icon;
  }

  const dot = name.lastIndexOf(".");
  // A leading dot is part of the name, not an extension: `.gitignore` is a
  // dotfile and reading `gitignore` as its extension would tint it as source.
  if (dot <= 0) return DEFAULT_LANG_ICON;
  return BY_EXTENSION[name.slice(dot + 1)] ?? DEFAULT_LANG_ICON;
}
