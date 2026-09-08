// What the Features panel shows, in the order it shows it, and the small facts
// each card carries. Pure over the store's arrays so a node test can hold the
// ordering and the counting to their rules.

import {
  featureIsOpen,
  matchesFilter,
  type FeatureFilter,
  type HarnessFeature,
} from "../harness/types";
import type { Job, Session, Workspace } from "../runtime/types";

export type FeatureScope = "Workspace" | "Project";

export type FeatureQuery = {
  features: HarnessFeature[];
  scope: FeatureScope;
  /** The checkout the scope is measured from; `null` when nothing is selected. */
  anchor: string | null;
  filter: FeatureFilter;
  /** The search box, already trimmed and lower-cased. */
  search: string;
};

/**
 * Whether a feature passes the scope.
 *
 * With no anchor the narrow scope admits everything: a scope measured from
 * nowhere is not a filter, it is an empty list. An anchor that exists filters
 * strictly, and a feature that names no checkout is in none of them.
 */
export function inScope(
  feature: HarnessFeature,
  scope: FeatureScope,
  anchor: string | null,
): boolean {
  if (scope === "Project" || anchor === null) return true;
  return feature.workspace_id === anchor;
}

/**
 * Whether a feature matches the search box.
 *
 * Reaches the id, the slug, the title and the spec the user typed — the set of
 * things somebody remembers about a feature.
 */
export function matchesSearch(feature: HarnessFeature, query: string): boolean {
  if (query === "") return true;
  if (String(feature.id).includes(query)) return true;
  if (feature.slug.toLowerCase().includes(query)) return true;
  return [feature.title, feature.spec_raw].some(
    (text) => text !== null && text.toLowerCase().includes(query),
  );
}

/**
 * The features the panel shows, open ones first and newest first within each
 * half.
 *
 * "Newest" is the id and not `created_at`, which the harness writes as a bare
 * date: two features registered on one afternoon are indistinguishable by it,
 * while ids are minted in order and never collide.
 *
 * Never sorts the caller's array in place — it is the store's list.
 */
export function visibleFeatures(query: FeatureQuery): HarnessFeature[] {
  return query.features
    .filter((feature) => matchesFilter(feature.status, query.filter))
    .filter((feature) => inScope(feature, query.scope, query.anchor))
    .filter((feature) => matchesSearch(feature, query.search))
    .slice()
    .sort((left, right) => {
      const open = Number(!featureIsOpen(left)) - Number(!featureIsOpen(right));
      return open !== 0 ? open : right.id - left.id;
    });
}

/** The steps run for one feature, oldest first — the order they ran in. */
export function featureSteps(jobs: Job[], feature: number): Job[] {
  return jobs.filter((job) => job.feature_id === feature);
}

/**
 * How many agents a feature has, without building any of their rows.
 *
 * A collapsed card needs the number and nothing else, and this runs once per
 * feature per redraw; resolving a title for every session on the way to a
 * count is a string allocation per agent for a fact that fits in a number.
 */
export function agentCount(sessions: Session[], jobs: Job[], feature: HarnessFeature): number {
  const root = feature.orchestrator_session_id;
  let live = 0;
  if (root !== null) {
    let found = false;
    let children = 0;
    for (const session of sessions) {
      if (session.id === root) found = true;
      else if (session.parent_session_id === root) children += 1;
    }
    // The tree is the orchestrator's; with the root gone from the store there
    // is no tree to count, which is what the roster itself does.
    live = found ? children + 1 : 0;
  }
  return live + featureSteps(jobs, feature.id).length;
}

/**
 * When one of this feature's steps last moved.
 *
 * Read off the jobs rather than the feature row, whose only instant is the
 * bare date it was registered on. A feature with no job on record has no
 * recency to report, and reports none.
 */
export function lastActivity(jobs: Job[], feature: number): string | null {
  let latest: string | null = null;
  for (const job of featureSteps(jobs, feature)) {
    const at = job.finished_at ?? job.started_at;
    if (latest === null || at > latest) latest = at;
  }
  return latest;
}

/** The checkout a feature occupies, named the way the rest of the shell names one. */
export function checkoutLabel(workspaces: Workspace[], feature: HarnessFeature): string | null {
  const id = feature.workspace_id;
  if (id === null) return null;
  const workspace = workspaces.find((item) => item.id === id);
  if (!workspace) return null;
  return workspace.display_name ?? workspace.branch ?? workspace.path;
}

/**
 * The facts on a card's meta line, in reading order and already filtered to
 * the ones this feature has.
 *
 * A list rather than a chain of optional children so the separators fall
 * between what is actually there: a dot before the first fact, or two in a
 * row, is what a chain produces the moment one of its links is missing.
 */
export function metaFacts(
  feature: HarnessFeature,
  sessions: Session[],
  jobs: Job[],
  workspaces: Workspace[],
  now: Date,
): string[] {
  const facts: string[] = [];
  // The slug only when the title is not already it: a card whose title fell
  // back to the slug would otherwise print it twice.
  if (feature.title !== null && feature.title !== "") facts.push(feature.slug);
  const agents = agentCount(sessions, jobs, feature);
  if (agents > 0) facts.push(plural(agents, "agent"));
  const checkout = checkoutLabel(workspaces, feature);
  if (checkout !== null) facts.push(checkout);
  const at = lastActivity(jobs, feature.id);
  if (at !== null) facts.push(`${relativeAge(at, now)} ago`);
  return facts;
}

/** The label/value table an expanded card shows under its agents. */
export function detailRows(
  feature: HarnessFeature,
  workspaces: Workspace[],
): { label: string; value: string }[] {
  const rows = [{ label: "Checkout", value: checkoutLabel(workspaces, feature) ?? "not claimed" }];
  // Printed as the harness wrote it: `created_at` is a bare date, so an age
  // computed from it would claim a precision the record does not have.
  if (feature.created_at) rows.push({ label: "Registered", value: feature.created_at });
  if (feature.review_rounds)
    rows.push({ label: "Review rounds", value: String(feature.review_rounds) });
  if (feature.gate_attempts)
    rows.push({ label: "Gate attempts", value: String(feature.gate_attempts) });
  if (feature.source_issue !== null) {
    rows.push({ label: "From issue", value: `#${feature.source_issue}` });
  }
  if (feature.blocked_reason) rows.push({ label: "Blocked", value: feature.blocked_reason });
  return rows;
}

/** How much of the file is on screen, phrased like the history panel's line. */
export function counts(shown: number, total: number, loading: boolean): string {
  const line = `${shown} shown · ${total} tracked`;
  return loading ? `${line} · loading…` : line;
}

/** What an empty list is empty *of*, which is a different sentence each time. */
export function emptyReason(
  project: string | null,
  initialized: boolean,
  total: number,
  search: string,
): { title: string; hint: string } {
  if (project === null) {
    return {
      title: "No project selected",
      hint: "Pick a session; the panel follows its repository.",
    };
  }
  if (!initialized) {
    return {
      title: "Harness not initialized",
      hint: "Run ./init.sh in this repository to start tracking features.",
    };
  }
  if (total === 0) {
    return {
      title: "No features yet",
      hint: "Use New to describe what to build. The harness registers it, writes the spec, and stops at your gate.",
    };
  }
  if (search !== "") {
    return { title: `Nothing matches “${search}”`, hint: "Clear the search to see the rest." };
  }
  return {
    title: "No features in this scope",
    hint: "Widen the scope or clear the status filter above.",
  };
}

/** `2 agents`, `1 agent`. */
export function plural(count: number, noun: string): string {
  return count === 1 ? `1 ${noun}` : `${count} ${noun}s`;
}

/** `text` cut to `max` characters, with an ellipsis when anything was cut. */
export function clip(text: string, max: number): string {
  const trimmed = text.trim();
  return [...trimmed].length > max ? `${[...trimmed].slice(0, max).join("")}…` : trimmed;
}

/** `5m`, `2h`, `3d` — the coarsest unit that is still true. */
export function relativeAge(at: string, now: Date): string {
  const minutes = Math.max(0, Math.floor((now.getTime() - new Date(at).getTime()) / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}
