// The parts of the launch-profile form with a right and a wrong answer (§13.4).
//
// Its own module and not helpers inside the component: a component module
// cannot be imported by a node test without pulling Solid's server build in
// with it.

import type { AgentProfile } from "../runtime/types";

export function newAgentProfile(
  providerId: string,
  id = globalThis.crypto.randomUUID(),
  createdAt = new Date().toISOString(),
): AgentProfile {
  return {
    id,
    provider_id: providerId,
    name: "",
    executable: null,
    config_dir: null,
    args: [],
    created_at: createdAt,
  };
}

/**
 * Command-line text into the arguments a launch passes one by one.
 *
 * Whitespace separates, so `--model opus` typed on one line is two arguments
 * and not one — which is how anyone who has used the CLI writes it. Quotes
 * group and a backslash escapes, because the alternative is that an argument
 * with a space in it (`--append-system-prompt "be terse"`) cannot be written
 * at all. That is the whole syntax: no shell runs any of this, so `$HOME`, `*`
 * and `;` reach the agent as the characters they are.
 */
export function parseArgs(text: string): { args: string[]; error: string | null } {
  const args: string[] = [];
  let current = "";
  let started = false;
  let quote: '"' | "'" | null = null;
  let escaped = false;

  for (const char of text) {
    if (escaped) {
      current += char;
      started = true;
      escaped = false;
      continue;
    }
    // Single quotes are literal, as in every shell: a Windows path or a regex
    // pasted between them stays what it was.
    if (char === "\\" && quote !== "'") {
      escaped = true;
      continue;
    }
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      started = true;
      continue;
    }
    if (/\s/.test(char)) {
      if (started) args.push(current);
      current = "";
      started = false;
      continue;
    }
    current += char;
    started = true;
  }
  if (escaped) return { args, error: "A trailing backslash escapes nothing." };
  if (quote) return { args, error: `An opening ${quote} is never closed.` };
  if (started) args.push(current);
  return { args, error: null };
}

/**
 * The inverse of {@link parseArgs}: what the form shows for saved arguments.
 *
 * One line per flag, with the values that follow it, because that is how the
 * command was written in the first place — `--model opus` reads as one thing
 * and is easier to edit than a single long line. Quotes appear only for an
 * argument that genuinely contains whitespace, and that argument gets a line of
 * its own so the quotes never sit in the middle of a readable line.
 */
export function formatArgs(args: readonly string[]): string {
  const lines: string[] = [];
  let quotedBefore = false;
  for (const arg of args) {
    const token = quoteArg(arg);
    const quoted = token !== arg;
    const ownLine = quoted || quotedBefore || arg.startsWith("-");
    if (ownLine || lines.length === 0) lines.push(token);
    else lines[lines.length - 1] += ` ${token}`;
    quotedBefore = quoted;
  }
  return lines.join("\n");
}

/**
 * Repair the profiles the previous editor saved, where every *line* became one
 * argument: `--model claude-opus-4-8` was stored as a single argument with a
 * space in it, which no CLI reads as a flag and its value.
 *
 * Only a flag-shaped argument is split, and only when it carries no `=`:
 * `--prompt=say hello` is one argument on purpose, and splitting it would break
 * a launch that works today.
 */
export function normalizeArgs(args: readonly string[]): string[] {
  return args.flatMap((arg) =>
    arg.startsWith("-") && !arg.includes("=") && /\s/.test(arg) ? arg.split(/\s+/) : [arg],
  );
}

/** One argument as the form spells it: bare when it can be, quoted when not. */
function quoteArg(arg: string): string {
  if (arg === "") return '""';
  if (!/[\s"'\\]/.test(arg)) return arg;
  return `"${arg.replace(/([\\"])/g, "\\$1")}"`;
}
