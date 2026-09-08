/**
 * Append-only event log per feature (12-factor: unified state + pause/resume).
 *
 * Each line in `harness/progress/events_<id>.jsonl` is one JSON object.
 * Agents append via `scripts/harness event <id> <type> [json]`.
 */

import { appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { PROGRESS } from "./harness.ts";

/** Known event types in the harness lifecycle. */
export type EventType =
  | "feature_registered"
  | "explore_done"
  | "spec_started"
  | "spec_ready"
  | "human_gate_opened"
  | "human_gate_resolved"
  | "context_bundled"
  | "impl_started"
  | "step_retried"
  | "step_stale"
  | "gate_bypassed"
  | "gate_failed"
  | "gate_passed"
  | "impl_done"
  | "review_started"
  | "review_verdict"
  | "feature_blocked"
  | "feature_done"
  | "feature_from_issue"
  | "feature_reopened"
  | "feature_committed";

export interface HarnessEvent {
  ts: string;
  type: EventType;
  [key: string]: unknown;
}

export function eventsPath(id: number): string {
  return join(PROGRESS, `events_${id}.jsonl`);
}

export function gatePath(id: number): string {
  return join(PROGRESS, `gate_${id}.md`);
}

export function contextPath(id: number): string {
  return join(PROGRESS, `context_${id}.md`);
}

/** Reads every event for a feature (empty array if the log does not exist). */
export function readEvents(id: number): HarnessEvent[] {
  try {
    const text = readFileSync(eventsPath(id), "utf8").trim();
    if (!text) return [];
    return text.split("\n").map((line) => JSON.parse(line) as HarnessEvent);
  } catch {
    return [];
  }
}

/** Appends one event line. Creates the file if needed. */
export function appendEvent(id: number, event: Omit<HarnessEvent, "ts"> & { ts?: string }): void {
  const { ts, ...rest } = event;
  const row: HarnessEvent = { ts: ts ?? new Date().toISOString(), ...rest };
  appendFileSync(eventsPath(id), `${JSON.stringify(row)}\n`);
}

/** Gate markdown written when a feature reaches `spec_ready`. */
export function gateTemplate(opts: {
  id: number;
  slug: string;
  title?: string;
  requirementCount?: number;
  crates?: string[];
  designNote?: string;
  specDir: string;
}): string {
  const crates = opts.crates?.length ? opts.crates.join(", ") : "(not declared)";
  const reqs =
    opts.requirementCount !== undefined ? String(opts.requirementCount) : "(see requirements.md)";
  const note = opts.designNote ?? "See design.md for the discarded alternative.";
  const title = opts.title ?? opts.slug;
  return `# Human gate — spec approval

**Feature:** ${opts.id} (${opts.slug})
**Title:** ${title}

**Question:** Do you approve this spec for implementation?

**Context:**
- Requirements: ${reqs}
- Crates: ${crates}
- Design note: ${note}

**Options:** \`approve\` | \`revise\` | \`block\`

**Spec:** \`${opts.specDir}/\`

---
*Resolve with \`/feature-go ${opts.id}\` (approve) or ask for changes before implementing.*
`;
}

/** Context bundle the lead writes before launching the implementer. */
export function contextTemplate(opts: {
  id: number;
  slug: string;
  specRaw: string;
  crates?: string[];
  explorePaths?: string[];
  invariantHints?: string[];
}): string {
  const explores =
    opts.explorePaths?.map((p) => `- \`${p}\``).join("\n") ?? "- (none — trivial feature)";
  const invariants =
    opts.invariantHints?.map((h) => `- ${h}`).join("\n") ??
    "- Read AGENTS.md § Boundaries And Invariants for the declared crates.";
  return `# Context bundle — feature ${opts.id} (${opts.slug})

## spec_raw (verbatim)

${opts.specRaw}

## Crates

${opts.crates?.join(", ") ?? "(not declared)"}

## Explore artefacts (read highlights, not full re-research)

${explores}

## Invariants to respect

${invariants}

## Implementer checklist

1. Follow \`harness/specs/${opts.id}-${opts.slug}/tasks.md\` in order.
2. Map every \`R<n>\` to a test or documented manual case in \`impl_${opts.id}.md\`.
3. Run \`scripts/dev check\` before handing off to review (max gate attempts enforced).
`;
}

/**
 * Cross-checks the event log against `features.json` status.
 * Returns human-readable error strings (empty = OK).
 */
export function validateEventCoherence(
  id: number,
  status: string,
  events: HarnessEvent[],
): string[] {
  const errors: string[] = [];
  if (events.length === 0) return errors;

  const types = new Set(events.map((e) => e.type));
  const last = events[events.length - 1]!;

  if (status === "spec_ready") {
    if (!types.has("spec_ready") && !types.has("human_gate_opened")) {
      errors.push(`feature ${id}: events log missing spec_ready/human_gate_opened for status spec_ready`);
    }
  }

  if (status === "in_progress") {
    if (!types.has("impl_started") && !types.has("context_bundled")) {
      errors.push(`feature ${id}: events log missing impl_started/context_bundled for in_progress`);
    }
  }

  if (status === "in_review") {
    if (!types.has("impl_done")) {
      errors.push(`feature ${id}: events log missing impl_done for in_review`);
    }
  }

  if (status === "done") {
    const approved = events.some(
      (e) => e.type === "review_verdict" && String(e.verdict).toUpperCase() === "APPROVED",
    );
    if (!approved && !types.has("feature_done")) {
      errors.push(`feature ${id}: events log missing review_verdict APPROVED for done`);
    }
  }

  if (status === "blocked" && last.type !== "feature_blocked" && !types.has("gate_failed")) {
    errors.push(`feature ${id}: blocked status but last event is '${last.type}', expected feature_blocked or gate_failed`);
  }

  return errors;
}
