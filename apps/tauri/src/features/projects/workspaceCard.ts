// What a checkout row in the rail says about itself: is anything running
// here, is the tree clean, is there a PR. Pure, so it is tested without a DOM;
// the markup lives in `ProjectsView.tsx`.

import type { SessionWork } from "../sessions/work";
import type { WorkspacePullRequest } from "./tree";

/**
 * How the states of the sessions in one checkout outrank each other.
 *
 * A card shows one marker for what could be five agents, so the loudest answer
 * wins: a question waiting behind a folded card is the one thing the rail must
 * never bury, and a crash outranks anything still healthy. `exited` is last —
 * a finished session says nothing about a checkout that also has a live one.
 */
const WORK_RANK: SessionWork[] = [
  "needs-you",
  "failed",
  "working",
  "starting",
  "idle",
  "running",
  "exited",
];

export function rollupWork(works: SessionWork[]): SessionWork | null {
  for (const candidate of WORK_RANK) {
    if (works.includes(candidate)) return candidate;
  }
  return null;
}

/** `↑2 ↓1` for a branch that has diverged, `null` when it has not. */
export function syncLabel(ahead: number | null, behind: number | null): string | null {
  const parts: string[] = [];
  if (ahead) parts.push(`↑${ahead}`);
  if (behind) parts.push(`↓${behind}`);
  return parts.length > 0 ? parts.join(" ") : null;
}

export type PrTone = "neutral" | "good" | "warn" | "bad";

/** A draft reads as neutral: it is open, but nobody is being asked anything. */
export function prTone(pr: WorkspacePullRequest): PrTone {
  if (pr.draft) return "neutral";
  switch (pr.decision) {
    case "Approved":
      return "good";
    case "ChangesRequested":
      return "bad";
    case "ReviewRequired":
      return "warn";
    default:
      return "neutral";
  }
}

export function prLabel(pr: WorkspacePullRequest): string {
  const state = pr.draft ? "draft" : reviewWord(pr.decision);
  return state ? `#${pr.number} · ${state} · ${pr.title}` : `#${pr.number} · ${pr.title}`;
}

function reviewWord(decision: string | null): string | null {
  switch (decision) {
    case "Approved":
      return "approved";
    case "ChangesRequested":
      return "changes requested";
    case "ReviewRequired":
      return "review required";
    default:
      return null;
  }
}
