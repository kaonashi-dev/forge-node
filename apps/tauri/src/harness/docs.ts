// Where each of a feature's markdown artefacts lives, and which of them a
// given card offers. Pure paths and lists so a node test can hold them to the
// layout `scripts/harness` writes.

import type { HarnessArtifactKind } from "./types";

export type DocTab = { kind: HarnessArtifactKind; label: string };

/** The artefacts a gate decision rests on, in the order they are written. */
export const GATE_DOCS: DocTab[] = [
  { kind: "Gate", label: "Gate" },
  { kind: "Requirements", label: "Requirements" },
  { kind: "Design", label: "Design" },
  { kind: "Tasks", label: "Tasks" },
];

/**
 * What a feature has written about itself once the spec is approved.
 *
 * The reviewer's verdict leads: a rejected feature says `blocked` in a badge,
 * and the paragraph explaining why is in `review_<id>.md`.
 */
export const RUN_DOCS: DocTab[] = [
  { kind: "Review", label: "Review" },
  { kind: "Impl", label: "Impl notes" },
  { kind: "Requirements", label: "Requirements" },
  { kind: "Design", label: "Design" },
  { kind: "Tasks", label: "Tasks" },
  { kind: "Context", label: "Context" },
  { kind: "Current", label: "Current" },
];

/**
 * Where a given artefact lives, spelled the way `harness/` spells it.
 *
 * Named in full so a missing document points at the file that is missing: the
 * path is what a person greps for, and the spec documents are under
 * `specs/<id>-<slug>/`, where the slug is the only part not derivable from the
 * id.
 */
export function docPath(kind: HarnessArtifactKind, feature: number, slug: string): string {
  switch (kind) {
    case "Gate":
      return `harness/progress/gate_${feature}.md`;
    case "Review":
      return `harness/progress/review_${feature}.md`;
    case "Impl":
      return `harness/progress/impl_${feature}.md`;
    case "Context":
      return `harness/progress/context_${feature}.md`;
    case "Current":
      return `harness/progress/current_${feature}.md`;
    case "Requirements":
      return `harness/specs/${feature}-${slug}/requirements.md`;
    case "Design":
      return `harness/specs/${feature}-${slug}/design.md`;
    case "Tasks":
      return `harness/specs/${feature}-${slug}/tasks.md`;
  }
}

/** What goes where the document would have been, when there is none. */
export function missingDocNote(path: string, error: string | null): string {
  return error === null ? `${path} is missing or empty.` : `${path} could not be read: ${error}`;
}
