// Which of the last read's pull requests the panel shows.
//
// The daemon already asked GitHub the three questions that matter — assigned,
// review requested, authored — and ships the answers as `PullRequest.relations`
// (§16.6). Filtering is therefore a read over data already in hand, not a
// second round trip: a scope the user picks must not cost a `gh` subprocess.
//
// Its own module so a node test can import it without Solid's server build.

import type { PullRequest, PullRequestFailure } from "../runtime/types";

/** `app_state` key holding the last scope, so the panel opens where it was. */
export const PR_SCOPE_KEY = "ui.pr_scope";

export type PrScope = "all" | "assigned" | "review_requested" | "authored";

export const PR_SCOPES: readonly PrScope[] = [
  "all",
  "assigned",
  "review_requested",
  "authored",
] as const;

/** What the segmented control says. Short: four of these share one row. */
export function scopeLabel(scope: PrScope): string {
  switch (scope) {
    case "assigned":
      return "Assigned";
    case "review_requested":
      return "To review";
    case "authored":
      return "Mine";
    default:
      return "All";
  }
}

/** The empty state, which has to say *why* the list is empty. */
export function scopeEmptyCopy(scope: PrScope): string {
  switch (scope) {
    case "assigned":
      return "No open pull request is assigned to you.";
    case "review_requested":
      return "Nobody has asked you for a review.";
    case "authored":
      return "You have no open pull requests.";
    default:
      return "No open pull requests in the last read.";
  }
}

/**
 * Parse a remembered scope.
 *
 * Anything unknown degrades to "all" rather than throwing, the same way the
 * default-agent preference does: `app_state` is untyped text by design, and a
 * value written by a newer build must not break this one.
 */
export function parseScope(value: string | null | undefined): PrScope {
  const trimmed = (value ?? "").trim();
  return (PR_SCOPES as readonly string[]).includes(trimmed) ? (trimmed as PrScope) : "all";
}

function inScope(pr: PullRequest, scope: PrScope): boolean {
  switch (scope) {
    case "assigned":
      return pr.relations.assigned;
    case "review_requested":
      return pr.relations.review_requested;
    case "authored":
      return pr.relations.authored;
    default:
      return true;
  }
}

/**
 * Case-insensitive match over the fields a person would search by.
 *
 * The body is deliberately not searched: it arrives truncated, so a hit or a
 * miss in it would depend on where the daemon cut the text.
 */
function matches(pr: PullRequest, needle: string): boolean {
  const haystack = [
    pr.title,
    pr.repository,
    pr.author,
    pr.head_ref,
    pr.base_ref,
    `#${pr.number}`,
    ...pr.labels.map((label) => label.name),
  ]
    .join(" ")
    .toLowerCase();
  return haystack.includes(needle);
}

/**
 * The rows to draw: this project's first, then everything else.
 *
 * A list that mixes repositories is one you have to read before you can click
 * in it, and the checkout on screen is the repository the reader is thinking
 * about. Within each half the host's own order is kept — it is by recency, and
 * re-sorting it here would be a second opinion with less information.
 */
export function filterPullRequests(
  pullRequests: readonly PullRequest[],
  scope: PrScope,
  query: string,
  project: string | null,
): { mine: PullRequest[]; others: PullRequest[] } {
  const needle = query.trim().toLowerCase();
  const kept = pullRequests.filter(
    (pr) => inScope(pr, scope) && (needle === "" || matches(pr, needle)),
  );
  return {
    mine: kept.filter((pr) => project !== null && pr.project_id === project),
    others: kept.filter((pr) => project === null || pr.project_id !== project),
  };
}

/** How many of the last read's pull requests fall in each scope, for the tabs. */
export function scopeCounts(pullRequests: readonly PullRequest[]): Record<PrScope, number> {
  return {
    all: pullRequests.length,
    assigned: pullRequests.filter((pr) => pr.relations.assigned).length,
    review_requested: pullRequests.filter((pr) => pr.relations.review_requested).length,
    authored: pullRequests.filter((pr) => pr.relations.authored).length,
  };
}

/**
 * The sentence a person reads for one failed host.
 *
 * The daemon ships a classified `kind` precisely so this is written here: its
 * own `message` is the CLI's words — an install path, a config key, a shell
 * command — none of which the user asked to learn. That text stays available
 * as a tooltip for whoever is debugging.
 *
 * An unrecognized kind falls back to the daemon's text rather than to silence:
 * a newer daemon must still be able to say something went wrong.
 */
export function failureCopy(failure: PullRequestFailure): string {
  const host = failure.host?.trim() ?? "";
  const repository = failure.repository?.trim() ?? "";
  switch (failure.kind) {
    case "CliMissing":
      return "Forge could not find the GitHub CLI, so pull requests cannot be read.";
    case "NotSignedIn":
      return `Forge is not signed in to ${host || "the host"}.`;
    case "TimedOut":
      // The host is the subject, so a missing one takes the capitalized
      // stand-in rather than opening the sentence in lower case.
      return host ? `${host} did not answer in time.` : "The host did not answer in time.";
    case "HostRefused":
      return host ? `${host} could not be read.` : "The host could not be read.";
    case "RepositoryRefused":
      if (repository) return `${repository} could not be read.`;
      return host
        ? `A repository on ${host} could not be read.`
        : "A repository could not be read.";
    default:
      return failure.message;
  }
}
