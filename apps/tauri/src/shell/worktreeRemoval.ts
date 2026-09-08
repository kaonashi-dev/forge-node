const PRECHECK_REASONS = [
  ["running_sessions=true", "One or more sessions are still running in it."],
  ["dirty=true", "It has uncommitted changes."],
  ["merge_or_rebase=true", "A merge or rebase is in progress."],
] as const;

/** Turn the daemon's exhaustive precheck into text meant for a person. */
export function describeWorktreeRemovalBlock(reason: string): string[] {
  const descriptions = PRECHECK_REASONS.filter(([token]) => reason.includes(token)).map(
    ([, description]) => description,
  );
  return descriptions.length > 0 ? descriptions : [reason];
}
