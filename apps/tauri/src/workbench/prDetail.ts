// What the pull request tab says about one pull request, as data.
//
// Its own module so a node test can import it without Solid's server build —
// the split `prFilters` already uses. Everything here answers from the row the
// daemon cached: the tab is a read of the last refresh and never a request.

import type { PullRequest, PullRequestRelations } from "../runtime/types";

export type ChipTone = "neutral" | "good" | "warn" | "bad";

export type Chip = { label: string; tone: ChipTone };

/** Open or draft — the daemon lists open pull requests, so there is no third. */
export function stateChip(pr: PullRequest): Chip {
  return pr.is_draft ? { label: "Draft", tone: "neutral" } : { label: "Open", tone: "good" };
}

/**
 * The host's review verdict, or `null` when it has none.
 *
 * An unknown value is shown rather than dropped: `ReviewDecision` is open by
 * design, and a verdict this build has not heard of is still the answer to
 * "can this merge".
 */
export function decisionChip(decision: string | null): Chip | null {
  const value = (decision ?? "").trim();
  switch (value) {
    case "":
      return null;
    case "Approved":
      return { label: "Approved", tone: "good" };
    case "ChangesRequested":
      return { label: "Changes requested", tone: "bad" };
    case "ReviewRequired":
      return { label: "Review required", tone: "warn" };
    default:
      return { label: humanize(value), tone: "neutral" };
  }
}

/** `ChangesRequested` → `Changes requested`. */
function humanize(value: string): string {
  const spaced = value
    .replace(/([a-z\d])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .toLowerCase()
    .trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/**
 * How the viewer is involved, in the order that decides what to do next.
 *
 * The daemon asked the host all three questions in the query it already ran
 * (§16.6 is the same data the scope chips filter on), so this costs nothing.
 */
export function relationChips(relations: PullRequestRelations): Chip[] {
  const chips: Chip[] = [];
  if (relations.review_requested) chips.push({ label: "Your review requested", tone: "warn" });
  if (relations.assigned) chips.push({ label: "Assigned to you", tone: "neutral" });
  if (relations.authored) chips.push({ label: "You opened this", tone: "neutral" });
  return chips;
}

/** `hellopay-tech/hellopay-backend` → its two halves, for the trail. */
export function splitRepository(repository: string): { owner: string | null; name: string } {
  const at = repository.lastIndexOf("/");
  if (at === -1) return { owner: null, name: repository };
  return { owner: repository.slice(0, at), name: repository.slice(at + 1) };
}

/**
 * A forge label's colour as CSS, or `null` for the theme's own.
 *
 * Six hex digits and nothing else: the value comes off the network and lands
 * in a `style` attribute, and it is a dot rather than a fill precisely so no
 * contrast has to be computed against text nobody here chose.
 */
export function labelColor(color: string | null | undefined): string | null {
  const value = (color ?? "").trim().replace(/^#/, "");
  return /^[0-9a-f]{6}$/i.test(value) ? `#${value}` : null;
}

/** `36 files changed`, `1 file changed`, `No files changed`. */
export function fileSummary(count: number): string {
  if (count <= 0) return "No files changed";
  return count === 1 ? "1 file changed" : `${count} files changed`;
}

/** `4 comments`, `1 comment`, `No comments`. */
export function commentSummary(count: number): string {
  if (count <= 0) return "No comments";
  return count === 1 ? "1 comment" : `${count} comments`;
}

/** The one letter that stands in for a face nobody downloaded. */
export function monogram(name: string): string {
  return ([...name.trim()][0] ?? "?").toUpperCase();
}

/** The date a person reads, from an ISO timestamp the host wrote. */
export function readableDate(iso: string): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? iso
    : at.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}
