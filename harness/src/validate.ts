#!/usr/bin/env bun
/**
 * Validates the coherence of harness/features.json.
 *
 * Run from `init.sh` (block 3) and from the session-close guard. Prints
 * [OK]/[FAIL] blocks and exits with 1 if the state is incoherent.
 */

import {
  ACTIVE,
  slotOf,
  FEATURES,
  SLUG_RE,
  SPECS,
  SPEC_FILES,
  SPEC_REQUIRED,
  isFile,
  parseVerdict,
  progressFile,
  rel,
  type Feature,
  type HarnessData,
} from "./harness.ts";
import {
  contextPath,
  gatePath,
  readEvents,
  validateEventCoherence,
} from "./events.ts";
import { join } from "node:path";

const IN_PROGRESS_STATUSES = new Set(["in_progress", "in_review"]);
/** Statuses a step runs under; `pending` is the spec step's. */
const RUNNING = new Set(["pending", "in_progress", "in_review"]);

export async function validate(): Promise<number> {
  const errors: string[] = [];
  const fail = (msg: string): void => {
    errors.push(msg);
    console.log(`[FAIL]  ${msg}`);
  };

  let data: HarnessData;
  try {
    data = JSON.parse(await Bun.file(FEATURES).text()) as HarnessData;
  } catch (exc) {
    if ((exc as NodeJS.ErrnoException)?.code === "ENOENT") {
      console.log(`[FAIL]  missing ${rel(FEATURES)}`);
    } else {
      console.log(`[FAIL]  features.json is not valid JSON: ${(exc as Error).message}`);
    }
    return 1;
  }

  const rules = data.rules ?? {};
  const valid = new Set(rules.valid_status ?? []);
  const maxRounds = rules.max_review_rounds ?? 2;
  const maxGateAttempts = rules.max_gate_attempts ?? 3;
  const requireGate = rules.require_gate_file ?? true;
  const requireContext = rules.require_context_bundle ?? true;
  const features: Feature[] = data.features ?? [];

  if (valid.size === 0) {
    fail("rules.valid_status empty or missing");
    return 1;
  }

  const seenIds = new Set<number>();
  /** Active features by the checkout they occupy. */
  const bySlot = new Map<string, string[]>();

  for (const f of features) {
    const fid = f.id;
    const slug = f.slug ?? "";
    const status = f.status;
    const where = `feature ${fid} (${slug})`;

    if (seenIds.has(fid)) fail(`duplicate id: ${fid}`);
    seenIds.add(fid);

    if (!SLUG_RE.test(slug)) fail(`${where}: slug missing or not kebab-case`);

    if (!valid.has(status)) {
      fail(`${where}: invalid status '${status}'`);
      continue;
    }

    if (ACTIVE.has(status)) {
      const slot = slotOf(f);
      bySlot.set(slot, [...(bySlot.get(slot) ?? []), where]);
    }

    const rounds = f.review_rounds ?? 0;
    if (rounds > maxRounds) {
      fail(`${where}: review_rounds=${rounds} exceeds max_review_rounds=${maxRounds}`);
    }

    const gateAttempts = f.gate_attempts ?? 0;
    if (gateAttempts > maxGateAttempts) {
      fail(`${where}: gate_attempts=${gateAttempts} exceeds max_gate_attempts=${maxGateAttempts}`);
    }

    // Attempts are written by `harness_service::transition`, never by an agent.
    // Their *count* is not checked: `max_step_attempts` is the daemon's budget
    // for automatic restarts, and a human `RetryStep` is a legal attempt past
    // it. A *live* attempt is checked — one on a feature nothing is running is
    // what a daemon restart leaves behind, and the daemon settles those on
    // boot, so one surviving here is state written by hand. `pending` counts
    // as running: the spec step runs under it.
    const attempts = f.attempts ?? [];
    if (!RUNNING.has(status) && attempts.some((a) => a.settled_at === undefined)) {
      fail(`${where}: status '${status}' but an attempt is still recorded as running`);
    }

    if (status === "blocked" && (f.blocked_reason ?? "").trim() === "") {
      fail(`${where}: 'blocked' without a blocked_reason`);
    }

    if (SPEC_REQUIRED.has(status)) {
      const dir = join(SPECS, `${fid}-${slug}`);
      const missing = SPEC_FILES.filter((n) => !isFile(join(dir, n)));
      if (missing.length > 0) {
        fail(`${where}: status '${status}' but ${rel(dir)}/{${missing.join(",")}} is missing`);
      }
    }

    if (requireGate && SPEC_REQUIRED.has(status) && !isFile(gatePath(fid))) {
      fail(`${where}: status '${status}' but ${rel(gatePath(fid))} is missing`);
    }

    if (requireContext && IN_PROGRESS_STATUSES.has(status) && !isFile(contextPath(fid))) {
      fail(`${where}: status '${status}' but ${rel(contextPath(fid))} is missing`);
    }

    const events = readEvents(fid);
    for (const msg of validateEventCoherence(fid, status, events)) {
      fail(msg);
    }

    if (status === "done") {
      const review = progressFile("review", fid);
      if (!isFile(review)) {
        fail(`${where}: 'done' without ${rel(review)}`);
      } else if (parseVerdict(await Bun.file(review).text()) !== "APPROVED") {
        fail(`${where}: 'done' but ${rel(review)} has no line that is exactly APPROVED`);
      }
    }
  }

  // One active feature per checkout, not per repository: two worktrees of the
  // same project are two working trees and may each carry one. Features with
  // no checkout recorded share a single slot, which is what the rule meant
  // back when Forge did not name one.
  for (const [slot, where] of bySlot) {
    if (where.length > 1) {
      fail(`${where.length} features active at once in ${slot} (maximum 1): ${where.join(", ")}`);
    }
  }

  if (errors.length > 0) return 1;

  const counts = new Map<string, number>();
  for (const f of features) counts.set(f.status, (counts.get(f.status) ?? 0) + 1);
  const summary =
    [...counts.entries()]
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([k, v]) => `${k}: ${v}`)
      .join(", ") || "no features";

  console.log(`[OK]    features.json coherent (${features.length} features — ${summary})`);
  return 0;
}

if (import.meta.main) {
  process.exit(await validate());
}
