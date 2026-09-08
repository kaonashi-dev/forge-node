/**
 * Shared model and paths of the subagent harness.
 *
 * The state lives on disk (`features.json`, `progress/`, `specs/`), not in chat.
 */

import { spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

/** The checkout this copy runs from: harness/src/ → harness/ → checkout. */
export const CHECKOUT = resolve(HERE, "..", "..");

/**
 * The checkout that owns the harness state.
 *
 * A worktree is a second checkout of the same repository, and the harness is
 * one state machine per *repository*: two features running side by side in two
 * worktrees still share one `features.json` and one id sequence. Forge resolves
 * the very same thing daemon-side from the project root (ADR-012), so the two
 * ends have to agree or the GUI reads a different file than the agent writes.
 *
 * `git rev-parse --git-common-dir` is that agreement — from any worktree it
 * answers the *main* checkout's `.git`. `FORGE_HARNESS_ROOT`, which the daemon
 * exports into every session it starts, wins when it is set, and this checkout
 * is the fallback for a copy that is not in a git repository at all.
 */
function harnessRoot(): string {
  const forced = process.env.FORGE_HARNESS_ROOT?.trim();
  if (forced) return resolve(forced);
  const probe = spawnSync("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], {
    cwd: CHECKOUT,
    encoding: "utf8",
  });
  const common = probe.status === 0 ? probe.stdout.trim() : "";
  // `<main>/.git` → `<main>`; a bare or unusual layout is left alone.
  return common.endsWith("/.git") ? dirname(common) : CHECKOUT;
}

/** Repo root that holds `harness/`, shared by every worktree of the project. */
export const ROOT = harnessRoot();
export const HARNESS = join(ROOT, "harness");
export const FEATURES = join(HARNESS, "features.json");
export const PROGRESS = join(HARNESS, "progress");
export const SPECS = join(HARNESS, "specs");

/**
 * The "what am I doing right now" note of one feature.
 *
 * Per feature rather than one shared `current.md`, because two features can be
 * in flight at once — one per checkout — and they would otherwise overwrite
 * each other's note. The shared file is still read when a feature has no note
 * of its own, so state written before the split is not lost.
 */
export function currentPath(id: number): string {
  const perFeature = join(PROGRESS, `current_${id}.md`);
  if (existsSync(perFeature)) return perFeature;
  const shared = join(PROGRESS, "current.md");
  return existsSync(shared) ? shared : perFeature;
}

export type Status =
  | "pending"
  | "spec_ready"
  | "in_progress"
  | "in_review"
  | "done"
  | "blocked";

export interface Feature {
  id: number;
  slug: string;
  title?: string;
  spec_raw?: string;
  crates?: string[];
  acceptance?: string[];
  status: Status;
  review_rounds?: number;
  /** Failed `scripts/dev check` attempts during implementation (factor 9). */
  gate_attempts?: number;
  /** GitHub issue number when registered via `from-issue`. */
  source_issue?: number;
  created_at?: string;
  /**
   * The checkout the feature is being implemented in, as Forge knows it.
   * Absent on features registered from the CLI, which name no checkout.
   */
  workspace_id?: string;
  /** That checkout's path, for reading this file by hand. */
  workspace_path?: string;
  /** Bumped by every daemon-side transition; a stale one makes a decision a no-op. */
  revision?: number;
  /** Every run of every step, in order. Written by the daemon's transition table. */
  attempts?: Attempt[];
  /** Why the feature is blocked, mirroring the last `feature_blocked` event. */
  blocked_reason?: string;
  [key: string]: unknown;
}

/**
 * One run of one step — the harness's Dispatch row.
 *
 * A retry is a *new* attempt and not a mutation of the previous one, so the
 * trail reads as "the implementer was started three times" rather than as "the
 * implementer is on attempt 3". An attempt with no `settled_at` and no live job
 * is a step that died with its daemon, which is exactly what makes restart
 * recovery possible.
 */
export interface Attempt {
  step: "Spec" | "Implement" | "Review";
  n: number;
  job?: string;
  provider?: string;
  transport?: string;
  started_at: string;
  settled_at?: string;
  outcome?: "succeeded" | "failed" | "cancelled" | "blocked" | "superseded";
  detail?: string;
}

export interface Rules {
  one_feature_at_a_time?: boolean;
  require_human_spec_approval?: boolean;
  require_green_gate_to_close?: boolean;
  max_review_rounds?: number;
  /** Auto-block the implementer after this many gate failures. */
  max_gate_attempts?: number;
  /** How many times the daemon restarts a step whose job failed, before blocking. */
  max_step_attempts?: number;
  /** Wall clock for one attempt; past it the daemon cancels the job. */
  max_step_minutes?: number;
  /** Silence after which an attempt is reported stale — a warning, never a kill. */
  stale_after_minutes?: number;
  /** Require `progress/gate_<id>.md` from `spec_ready` on. */
  require_gate_file?: boolean;
  /** Require `progress/context_<id>.md` from `in_progress` on. */
  require_context_bundle?: boolean;
  valid_status?: string[];
}

export interface HarnessData {
  project?: string;
  description?: string;
  rules?: Rules;
  features?: Feature[];
}

/** Statuses in which the spec must already exist on disk. */
export const SPEC_REQUIRED: ReadonlySet<string> = new Set([
  "spec_ready",
  "in_progress",
  "in_review",
  "done",
]);

/** Statuses that occupy a checkout's active slot. */
export const ACTIVE: ReadonlySet<string> = new Set(["in_progress", "in_review"]);

/**
 * The slot a feature occupies: its checkout, or the shared "no checkout" slot
 * for features registered outside Forge. One active feature per slot — which
 * is the whole of `one_feature_at_a_time` now that a project can be checked
 * out into several worktrees at once.
 */
export function slotOf(f: Feature): string {
  return f.workspace_id ?? "(no checkout)";
}

export const SPEC_FILES = ["requirements.md", "design.md", "tasks.md"] as const;

export const SLUG_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

/** Reads and parses `features.json`. Throws if it does not exist or is not valid JSON. */
export async function load(): Promise<HarnessData> {
  const text = await Bun.file(FEATURES).text();
  return JSON.parse(text) as HarnessData;
}

export function features(data: HarnessData): Feature[] {
  return data.features ?? [];
}

export function specDir(f: Pick<Feature, "id" | "slug">): string {
  return join(SPECS, `${f.id}-${f.slug}`);
}

export function progressFile(name: string, id: number): string {
  return join(PROGRESS, `${name}_${id}.md`);
}

export function isDir(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

export function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

export { existsSync };

/** Path relative to the repo root, with POSIX separators. */
export function rel(path: string): string {
  return relative(ROOT, path);
}

/** Padding by code points (not by UTF-16 units), like Python. */
function width(s: string): number {
  return [...s].length;
}

export function padEnd(s: string, n: number): string {
  const pad = n - width(s);
  return pad > 0 ? s + " ".repeat(pad) : s;
}

export function padStart(s: string, n: number): string {
  const pad = n - width(s);
  return pad > 0 ? " ".repeat(pad) + s : s;
}

/** What a review file said, once. */
export type Verdict = "APPROVED" | "CHANGES_REQUESTED" | "MISSING" | "AMBIGUOUS";

/**
 * Read the verdict out of a review file.
 *
 * A *line*, never a substring of the document. The rule here used to be
 * `text.includes("APPROVED")`, which passed on the word wherever it appeared —
 * including in `.claude/agents/reviewer.md`'s own format example, which prints
 * `**Verdict:** APPROVED | CHANGES_REQUESTED`, and in the ordinary prose of a
 * rejection ("cannot be approved").
 *
 * Kept in step with `parse_verdict` in `crates/daemon/src/harness_runner.rs`:
 * the daemon decides with it and this validates with it, so they have to agree
 * on what a review file says.
 */
export function parseVerdict(review: string): Verdict {
  let seen: Verdict | null = null;
  for (const line of review.split("\n")) {
    const verdict = verdictOnLine(line);
    if (!verdict) continue;
    if (seen === null) seen = verdict;
    // A reviewer that wrote both answered neither.
    else if (seen !== verdict) return "AMBIGUOUS";
  }
  return seen ?? "MISSING";
}

/**
 * The verdict a single line states, or `null` when it states none.
 *
 * Tolerates the markdown a reviewer actually writes — a bullet, a heading, an
 * emphasised `**Verdict:**` label — and then demands that what is left be
 * *exactly* one token. That last part is what makes the unfilled
 * `APPROVED | CHANGES_REQUESTED` template line match nothing.
 */
function verdictOnLine(line: string): "APPROVED" | "CHANGES_REQUESTED" | null {
  let text = line.trim().replace(/^[-*#>\s]+/, "");
  text = text.replace(/^[*_]+/, "").trim();
  const labelled = /^\**\s*verdict\s*\**\s*:(.*)$/i.exec(text);
  if (labelled) text = labelled[1];
  text = text.trim().replace(/^[*_`\s]+|[*_`\s]+$/g, "").trim();
  const token = text.toUpperCase();
  if (token === "APPROVED") return "APPROVED";
  // `REJECTED` is accepted beside `CHANGES_REQUESTED` because review files
  // written by earlier builds say it.
  if (token === "CHANGES_REQUESTED" || token === "REJECTED") return "CHANGES_REQUESTED";
  return null;
}
