// Ownership edges the frontend is allowed to write. See docs/frontend-architecture.md.
//
// Foundations may only reach down; features reach foundations, themselves, and
// the peer features listed in PEER_EDGES. `app/` reaches everything, and
// nothing below it reaches back.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

export type Source = { path: string; source: string };

export type Edge = {
  from: string;
  to: string;
  spec: string;
  kind: "static" | "dynamic" | "type";
};

const PEER_EDGES: Record<string, string[]> = {
  editor: ["files"],
  files: ["editor", "git", "harness", "settings", "terminal"],
  git: ["editor", "files", "pull-requests", "sessions", "settings"],
  harness: ["editor", "files", "sessions"],
  projects: ["git", "pull-requests", "sessions", "settings"],
  "pull-requests": ["editor", "files", "git", "harness", "sessions", "settings"],
  sessions: ["editor", "git", "settings", "terminal"],
  settings: ["harness", "projects", "sessions", "terminal"],
  terminal: ["files"],
};

const FOUNDATIONS = [
  "contracts",
  "state",
  "shared",
  "theme",
  "ui",
  "actions",
  "navigation",
  "runtime",
  "styles",
];

const ALLOWED: Record<string, RegExp[]> = {
  contracts: [/^contracts\//],
  runtime: [/^contracts\//, /^runtime\//],
  state: [/^contracts\//, /^state\//, /^runtime\/host\.ts$/],
  shared: [/^contracts\//, /^shared\//, /^theme\//, /^ui\//],
  theme: [/^contracts\//, /^theme\//],
  ui: [/^actions\//, /^contracts\//, /^theme\//, /^ui\//],
  actions: [/^actions\//, /^contracts\//, /^runtime\/host\.ts$/, /^state\//],
  navigation: [/^contracts\//, /^navigation\//, /^shared\//, /^state\//, /^theme\//],
};

const PRODUCTION = /\.(ts|tsx)$/;
const TEST = /\.test\.(ts|tsx)$/;
const GENERATED = /^contracts\/generated\//;

function relativize(path: string): string {
  return path.replace(/^\.\//, "");
}

const IMPORT_RE = /(?:^|\n)\s*(?:import|export)\s+(type\s+)?[^"'();]*?from\s*["']([^"']+)["']/g;
const SIDE_EFFECT_RE = /(?:^|\n)\s*import\s+["']([^"']+)["']/g;
const DYNAMIC_RE = /\bimport\s*\(\s*["']([^"']+)["']\s*\)/g;

function specifiers(source: string): { spec: string; kind: Edge["kind"] }[] {
  const out: { spec: string; kind: Edge["kind"] }[] = [];
  let match: RegExpExecArray | null;
  IMPORT_RE.lastIndex = 0;
  while ((match = IMPORT_RE.exec(source))) {
    out.push({ spec: match[2], kind: match[1] ? "type" : "static" });
  }
  SIDE_EFFECT_RE.lastIndex = 0;
  while ((match = SIDE_EFFECT_RE.exec(source))) out.push({ spec: match[1], kind: "static" });
  DYNAMIC_RE.lastIndex = 0;
  while ((match = DYNAMIC_RE.exec(source))) out.push({ spec: match[1], kind: "dynamic" });
  return out;
}

function ownerOf(path: string): string {
  const [first, second] = path.split("/");
  if (first === "features") return `features/${second}`;
  if (first === "main.tsx" || first === "app") return "app";
  return first;
}

function resolveSpec(from: string, spec: string, files: Set<string>): string | null {
  const [pathPart] = spec.split("?", 1);
  if (!pathPart.startsWith(".")) return null;
  const base = resolve(dirname(join("src", from)), pathPart);
  const relativeToRoot = relative("src", base).replaceAll("\\", "/");
  const candidates = [
    relativeToRoot,
    relativeToRoot + ".ts",
    relativeToRoot + ".tsx",
    join(relativeToRoot, "index.ts"),
    join(relativeToRoot, "index.tsx"),
  ].map(relativize);
  for (const candidate of candidates) if (files.has(candidate)) return candidate;
  return null;
}

export function graphOf(sources: Source[]): { edges: Edge[]; files: Set<string> } {
  const production = sources.filter(
    (file) => PRODUCTION.test(file.path) && !TEST.test(file.path) && !GENERATED.test(file.path),
  );
  const files = new Set(production.map((file) => relativize(file.path)));
  const edges: Edge[] = [];
  for (const file of production) {
    for (const { spec, kind } of specifiers(file.source)) {
      const target = resolveSpec(file.path, spec, files);
      if (target) edges.push({ from: relativize(file.path), to: target, spec, kind });
    }
  }
  return { edges, files };
}

export function violationsOf(sources: Source[]): string[] {
  const { edges } = graphOf(sources);
  const violations: string[] = [];
  for (const edge of edges) {
    const fromOwner = ownerOf(edge.from);
    const toOwner = ownerOf(edge.to);
    if (fromOwner === "app") continue;
    if (fromOwner.startsWith("features/")) {
      const feature = edge.from.split("/")[1];
      if (toOwner === `features/${feature}`) continue;
      if (FOUNDATIONS.includes(toOwner)) continue;
      if (toOwner.startsWith("features/") && PEER_EDGES[feature]?.includes(toOwner.slice(9))) {
        continue;
      }
      violations.push(`${edge.from} -> ${edge.to}: ${feature} may not import ${toOwner}`);
      continue;
    }
    const allowed = ALLOWED[fromOwner];
    if (allowed === undefined) {
      violations.push(`${edge.from} -> ${edge.to}: ${fromOwner}/ has no ownership rule`);
      continue;
    }
    if (!allowed.some((pattern) => pattern.test(edge.to))) {
      violations.push(`${edge.from} -> ${edge.to}: ${fromOwner}/ may not import ${toOwner}`);
    }
  }
  return violations;
}

function cyclesOf(edges: Edge[]): string[][] {
  const nodes = [...new Set(edges.flatMap((edge) => [edge.from, edge.to]))];
  const index = new Map(nodes.map((node, at) => [node, at]));
  const outgoing = new Map<string, string[]>();
  for (const edge of edges) {
    if (edge.kind === "type") continue;
    outgoing.set(edge.from, [...(outgoing.get(edge.from) ?? []), edge.to]);
  }
  let counter = 0;
  const stack: number[] = [];
  const onStack = new Uint8Array(nodes.length);
  const low = new Int32Array(nodes.length);
  const num = new Int32Array(nodes.length).fill(-1);
  const components: string[][] = [];
  const connect = (at: number) => {
    num[at] = low[at] = counter++;
    stack.push(at);
    onStack[at] = 1;
    for (const to of outgoing.get(nodes[at]) ?? []) {
      const next = index.get(to);
      if (next === undefined) continue;
      if (num[next] === -1) {
        connect(next);
        low[at] = Math.min(low[at], low[next]);
      } else if (onStack[next]) {
        low[at] = Math.min(low[at], num[next]);
      }
    }
    if (low[at] === num[at]) {
      const component: string[] = [];
      let node: number;
      do {
        node = stack.pop()!;
        onStack[node] = 0;
        component.push(nodes[node]);
      } while (node !== at);
      if (component.length > 1) components.push(component);
    }
  };
  for (let at = 0; at < nodes.length; at++) if (num[at] === -1) connect(at);
  return components;
}

export function cycles(sources: Source[]): string[][] {
  return cyclesOf(graphOf(sources).edges);
}

export function typeOnlyEdges(sources: Source[]): Edge[] {
  return graphOf(sources).edges.filter((edge) => edge.kind === "type");
}

function readSources(root: string): Source[] {
  const out: Source[] = [];
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) walk(path);
      else if (PRODUCTION.test(name)) {
        out.push({
          path: relative(root, path).replaceAll("\\", "/"),
          source: readFileSync(path, "utf8"),
        });
      }
    }
  };
  walk(root);
  return out;
}

if (import.meta.main) {
  const root = join(import.meta.dir, "../src");
  const sources = readSources(root);
  const violations = violationsOf(sources);
  const typeOnly = typeOnlyEdges(sources);
  const runtimeCycles = cycles(sources);
  if (typeOnly.length > 0) {
    console.log(`type-only edges: ${typeOnly.length} (reported, not cycle-checked)`);
  }
  for (const component of runtimeCycles) {
    console.error(`import cycle: ${component.join(" -> ")}`);
  }
  for (const violation of violations) console.error(`forbidden import: ${violation}`);
  if (violations.length > 0 || runtimeCycles.length > 0) {
    console.error(`\n${violations.length} forbidden import(s), ${runtimeCycles.length} cycle(s).`);
    process.exit(1);
  }
  console.log(`ok  boundaries: ${sources.length} modules, no forbidden imports or cycles`);
}
