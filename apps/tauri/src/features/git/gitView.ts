// What the Git panel says about a checkout, as data a test can import without
// Solid's server build.

import { parsePatch, type PatchRow } from "./diff/patch";
import type { DiffStatus, RebaseState } from "../../contracts/workbench";

/** Git's own porcelain letter: `U` is unmerged, so an untracked file is `?`. */
export function statusLetter(status: DiffStatus): string {
  switch (status) {
    case "Added":
      return "A";
    case "Modified":
      return "M";
    case "Deleted":
      return "D";
    case "Renamed":
      return "R";
    case "Untracked":
      return "?";
    case "Conflicted":
      return "U";
    default:
      return status.slice(0, 1).toUpperCase() || "?";
  }
}

export function statusWord(status: DiffStatus): string {
  switch (status) {
    case "Conflicted":
      return "unresolved conflict";
    case "Untracked":
      return "untracked";
    default:
      return status.toLowerCase();
  }
}

export type StepState = "done" | "current" | "pending";

/** Past this many commits the meter stops drawing one segment per commit. */
export const MAX_SEGMENTS = 20;

/**
 * The sequencer's position as meter segments plus the sentence that carries it.
 *
 * `null` when git did not report a position (a merge has none). A replay
 * longer than {@link MAX_SEGMENTS} is drawn proportionally, so a segment no
 * longer maps to one commit; the sentence is exact either way.
 */
export function rebaseSteps(
  state: Pick<RebaseState, "step" | "total"> | null,
): { segments: StepState[]; label: string } | null {
  const step = state?.step ?? null;
  const total = state?.total ?? null;
  if (step === null || total === null || total <= 0) return null;
  const clamped = Math.min(Math.max(step, 1), total);
  const count = Math.min(total, MAX_SEGMENTS);
  const current = Math.min(count, Math.max(1, Math.ceil((clamped / total) * count)));
  const segments = Array.from({ length: count }, (_, index): StepState => {
    if (index + 1 < current) return "done";
    return index + 1 === current ? "current" : "pending";
  });
  return { segments, label: `step ${clamped} of ${total}` };
}

/** First paragraph is the subject, the rest the body — the dialog's rule. */
export function commitMessage(text: string): { title: string; body: string } {
  const parts = text.trim().split("\n\n");
  return {
    title: parts[0]?.trim() ?? "",
    body: parts.slice(1).join("\n\n").trim(),
  };
}

/** `CreateCommit` stages the whole tree, so the button counts every change. */
export function commitLabel(files: number): string {
  if (files <= 0) return "Nothing to commit";
  return files === 1 ? "Commit 1 file" : `Commit ${files} files`;
}

export type HunkPreview = {
  header: string;
  rows: PatchRow[];
  /** Lines of this hunk past `limit`. */
  hiddenRows: number;
  /** Hunks after this one. */
  more: number;
};

/** The first hunk of a patch, cut to `limit` lines for the rail's preview. */
export function firstHunk(patch: string, limit = 12): HunkPreview | null {
  const rows = parsePatch(patch);
  const starts = rows.flatMap((row, index) => (row.kind === "hunk" ? [index] : []));
  const start = starts[0];
  if (start === undefined) return null;
  const end = starts[1] ?? rows.length;
  const header = rows[start]!.text.match(/^@@[^@]*@@/)?.[0] ?? rows[start]!.text;
  const body = rows.slice(start + 1, end).filter((row) => row.kind !== "meta");
  return {
    header,
    rows: body.slice(0, limit),
    hiddenRows: Math.max(0, body.length - limit),
    more: starts.length - 1,
  };
}

export function operationName(operation: string | null | undefined): string {
  switch (operation ?? "Rebase") {
    case "CherryPick":
      return "Cherry-pick";
    case "Rebase":
    case "Merge":
    case "Revert":
      return operation ?? "Rebase";
    default:
      return String(operation);
  }
}
