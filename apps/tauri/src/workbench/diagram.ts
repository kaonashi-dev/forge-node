// Mermaid diagrams for rendered Markdown. The engine is a chunk of its own,
// imported on the first diagram: it is several times the editor route's whole
// budget (docs/performance.md) and most documents never draw one.

import type { Mermaid, MermaidConfig } from "mermaid";
import { baseIsLight, type ThemeBaseId } from "../theme/tokens";

let engine: Promise<Mermaid> | undefined;
let configured: string | null = null;
let rendered = 0;

/** Whether a fenced block's language is drawn rather than quoted as code. */
export function isDiagram(lang: string | null): boolean {
  return lang?.toLowerCase() === "mermaid";
}

/**
 * The SVG markup for one diagram, or a rejection carrying mermaid's parse
 * error.
 *
 * `securityLevel: "strict"` is what makes the markup safe to insert: labels
 * are escaped, click handlers are dropped and the result goes through
 * DOMPurify before it is returned.
 */
export async function renderDiagram(source: string, base: ThemeBaseId): Promise<string> {
  const mermaid = await loadEngine();
  const config: MermaidConfig = {
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    ...themeFor(base),
  };
  const key = JSON.stringify(config);
  if (key !== configured) {
    mermaid.initialize(config);
    configured = key;
  }
  rendered += 1;
  const { svg } = await mermaid.render(`forge-diagram-${rendered}`, source);
  return svg;
}

function loadEngine(): Promise<Mermaid> {
  engine ??= import("mermaid").then(
    (module) => module.default,
    (error: unknown) => {
      // A chunk that failed to load is retried by the next diagram, not never.
      engine = undefined;
      throw error;
    },
  );
  return engine;
}

/**
 * Mermaid's theme, painted from the shell's own tokens.
 *
 * Its `base` theme derives every shade from a few colours with a parser that
 * reads hex and nothing newer, so a palette spelled any other way falls back
 * to the stock light or dark theme rather than to a half-broken one.
 */
function themeFor(base: ThemeBaseId): Pick<MermaidConfig, "theme" | "themeVariables"> {
  const dark = !baseIsLight(base);
  const style = getComputedStyle(document.documentElement);
  const token = (name: string): string => style.getPropertyValue(name).trim();
  const colors = {
    background: token("--bg-base"),
    mainBkg: token("--bg-raised"),
    primaryColor: token("--bg-raised"),
    primaryTextColor: token("--fg-default"),
    primaryBorderColor: token("--border-default"),
    secondaryColor: token("--bg-subtle"),
    tertiaryColor: token("--bg-subtle"),
    lineColor: token("--fg-muted"),
    textColor: token("--fg-default"),
  };
  if (!Object.values(colors).every((value) => /^#[0-9a-f]{3,8}$/i.test(value))) {
    return { theme: dark ? "dark" : "default" };
  }
  return {
    theme: "base",
    themeVariables: { darkMode: dark, fontFamily: token("--font-sans"), ...colors },
  };
}
