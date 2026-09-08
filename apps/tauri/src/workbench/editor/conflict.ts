// A6: what a fresh ReadFile means for the open buffer.
//
// Its own module because the wrong answer is a banner on every keystroke:
// a local edit is not a disk change, and only a new revision is.

export type DiskReadAction = "ignore" | "apply" | "conflict" | "matched";

export function actionForDiskRead(input: {
  seenRevision: string | undefined;
  revision: string;
  dirty: boolean;
  disk: string;
  mine: string;
}): DiskReadAction {
  // Same snapshot we already reconciled. Re-reading it (tree click, focus)
  // must not raise a conflict against the draft.
  if (input.seenRevision === input.revision) return "ignore";
  if (!input.dirty) return "apply";
  // Save re-reads the bytes we just wrote: the draft is now the disk.
  if (input.disk === input.mine) return "matched";
  return "conflict";
}
