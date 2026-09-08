// Workbench wire types — `domain::{diff, file, rebase}`.
//
// Every one of these is runtime-only on the daemon side: no column, no
// migration, no `Store` field. They are answers to a question the panel asked,
// which is why they live in their own store here rather than in `forgeStore`.

export type DiffStatus = "Added" | "Modified" | "Deleted" | "Renamed" | "Untracked" | string;

export type DiffFile = {
  path: string;
  status: DiffStatus;
  additions: number;
  deletions: number;
  patch: string;
  binary: boolean;
  /** The patch was over budget and dropped whole — half a patch is not a patch. */
  truncated: boolean;
};

export type WorkspaceDiff = {
  workspace_id: string;
  branch: string | null;
  files: DiffFile[];
  truncated: boolean;
};

/**
 * Why the base of a change read is what it is.
 *
 * Shown rather than hidden: `Unreachable` means a recorded baseline stopped
 * resolving — a rebase, an amend — and the numbers are against `HEAD` instead,
 * which is a much smaller story than the one the reader asked for.
 */
export type BaseOrigin = "Recorded" | "Missing" | "Unreachable" | string;

export type ChangeSummaryFile = {
  path: string;
  status: DiffStatus;
  additions: number;
  deletions: number;
  binary: boolean;
};

export type CommitLine = {
  short_id: string;
  subject: string;
};

export type ChangeSummary = {
  branch: string | null;
  base: string | null;
  files: ChangeSummaryFile[];
  commits: CommitLine[];
  /** Can exceed `commits.length`: the list is capped, the count is not. */
  commit_count: number;
  truncated: boolean;
};

export type SessionChanges = {
  session_id: string;
  origin: BaseOrigin;
  summary: ChangeSummary;
  /** Other live sessions writing this checkout; non-zero means these numbers
   *  can include work this session did not do. */
  sharing_sessions: number;
};

export type ReviewSession = {
  session_id: string;
  title: string;
  provider: string | null;
  active: boolean;
  base: string | null;
  commit_count: number;
  created_at: string;
  ended_at: string | null;
};

export type WorkspaceReview = {
  workspace_id: string;
  base: string | null;
  origin: BaseOrigin;
  sessions: ReviewSession[];
  commits: CommitLine[];
  diff: WorkspaceDiff;
};

export type SessionTranscript = {
  session_id: string;
  text: string;
  lines: number;
  truncated: boolean;
};

export type FileKind = "File" | "Directory" | string;

export type FileEntry = {
  path: string;
  kind: FileKind;
};

export type FileTree = {
  workspace_id: string;
  entries: FileEntry[];
  /** The scan hit its budget: shown as such, never as a complete listing. */
  truncated: boolean;
};

export type FileContents = {
  workspace_id: string;
  path: string;
  text: string;
  /** Required to write it back; a stale one is `PreconditionFailed`. */
  revision: string;
  language: string;
  binary: boolean;
  too_large: boolean;
};

export type SearchMatch = {
  path: string;
  line: number;
  column: number;
  text: string;
};

/**
 * `definition` is not a content search with a clever query: the daemon greps
 * for the bare word and decides what a declaration looks like, so no regex
 * that has to be right in eleven languages ever leaves this side.
 */
export type SearchKind = "name" | "content" | "definition";

export type SearchResults = {
  workspace_id: string;
  /** What was asked. An answer carries no request id; this is the only tag. */
  query: string;
  matches: SearchMatch[];
  truncated: boolean;
};

export type SequencerOp = "Rebase" | "Merge" | "CherryPick" | "Revert" | string;

export type ConflictFile = {
  path: string;
  /** The two-letter `git status` code; `UU`, `AA`, `DU`… */
  code: string;
};

export type RebaseState = {
  workspace_id: string;
  operation: SequencerOp | null;
  head: string | null;
  branch: string | null;
  onto: string | null;
  step: number | null;
  total: number | null;
  conflicts: ConflictFile[];
  truncated: boolean;
};

export type Branches = {
  branches: BranchRef[];
  remotes: Remote[];
  default_branch: string | null;
};

export type RefScope = "Local" | { Remote: { remote: string } };

export type BranchRef = {
  name: string;
  scope: RefScope;
  upstream: string | null;
  committed_at: string | null;
  subject: string | null;
  checked_out_in: string | null;
};

export type Remote = {
  name: string;
  url: string;
};

export type TokenTotals = {
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
  reasoning: number;
};

/**
 * Every billed token: input, output and both halves of the cache.
 *
 * `reasoning` is deliberately absent — it is already inside `output`, and
 * counting it twice is the easiest way to publish a wrong number. Mirrors
 * `TokenTotals::total` on the daemon side, which does not serialise it.
 */
export function tokenTotal(tokens: TokenTotals): number {
  return tokens.input + tokens.output + tokens.cache_write + tokens.cache_read;
}

export type ProviderAnalytics = {
  provider_id: string;
  tokens: TokenTotals;
  sessions: number;
  turns: number;
  cost_micros: number;
  unpriced_turns: number;
  top_model: string | null;
  worked_secs: number;
  first_activity: string | null;
  last_activity: string | null;
};

export type DailyUsage = {
  date: string;
  tokens: number;
};

export type UsageAnalytics = {
  providers: ProviderAnalytics[];
  daily: DailyUsage[];
  window_days: number;
  scanned: number;
  skipped: number;
  collected_at: string;
};

export function analyticsSessions(analytics: UsageAnalytics): number {
  return analytics.providers.reduce((sum, provider) => sum + provider.sessions, 0);
}

export function analyticsTurns(analytics: UsageAnalytics): number {
  return analytics.providers.reduce((sum, provider) => sum + provider.turns, 0);
}

export function analyticsWorkedSecs(analytics: UsageAnalytics): number {
  return analytics.providers.reduce((sum, provider) => sum + provider.worked_secs, 0);
}

export type JuvaKind = "CommitMessage" | "PullRequest" | "ChangeReview" | "Unknown";

export type ChangeFile = {
  path: string;
  status: string;
};

export type ChangeContext = {
  branch: string | null;
  default_branch: string | null;
  dirty: boolean;
  ahead: number | null;
  behind: number | null;
  files: ChangeFile[];
  patch: string;
  truncated: boolean;
};

export type JuvaDraft = {
  kind: JuvaKind;
  title: string;
  body: string;
  context: ChangeContext;
};

export function juvaAsEditable(draft: JuvaDraft): string {
  if (!draft.body.trim()) return draft.title;
  return `${draft.title.trimEnd()}\n\n${draft.body.trimEnd()}`;
}

/** A stopped sequencer is a read, never a remembered state. */
export function rebaseInProgress(state: RebaseState | null): boolean {
  return state?.operation != null;
}

/**
 * Whether `--continue` can move.
 *
 * Staging a path is what takes it off the conflict list, so this is derived
 * from the list the daemon just read — never from a count kept between reads.
 */
export function readyToContinue(state: RebaseState | null): boolean {
  return rebaseInProgress(state) && (state?.conflicts.length ?? 0) === 0;
}

export function diffTotals(diff: WorkspaceDiff | null): { additions: number; deletions: number } {
  return (diff?.files ?? []).reduce(
    (total, file) => ({
      additions: total.additions + file.additions,
      deletions: total.deletions + file.deletions,
    }),
    { additions: 0, deletions: 0 },
  );
}

export function summaryTotals(summary: ChangeSummary | null): {
  additions: number;
  deletions: number;
} {
  return (summary?.files ?? []).reduce(
    (total, file) => ({
      additions: total.additions + file.additions,
      deletions: total.deletions + file.deletions,
    }),
    { additions: 0, deletions: 0 },
  );
}

/** What the header says about a base the reader did not choose. */
export function baseNote(origin: BaseOrigin, base: string | null): string | null {
  if (origin === "Recorded") return null;
  if (origin === "Unreachable") {
    return "The recorded baseline no longer resolves — showing changes against HEAD.";
  }
  return base === null ? "No baseline was recorded — showing uncommitted changes." : null;
}
