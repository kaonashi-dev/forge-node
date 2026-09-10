export type SessionKind = "Shell" | "Agent" | string;

export type SessionState =
  | "Starting"
  | "Running"
  | "Orphaned"
  | { Exited?: { code: number | null; signal: number | null } }
  | { Failed?: { reason: string } }
  | string;

export type SessionTitle = {
  user: string | null;
  terminal: string | null;
};

/**
 * What a session is *for* (§8.2), mirroring `domain::SessionRole`.
 *
 * The harness cares: an orchestrator owns a feature and the children under it
 * do its steps, which is what the rail indents them by. `Custom` serializes as
 * an externally tagged variant, so it arrives as an object.
 */
export type SessionRole =
  | "Generic"
  | "Orchestrator"
  | "Planner"
  | "Researcher"
  | "Executor"
  | "Reviewer"
  | "Tester"
  | { Custom: string };

export type Session = {
  id: string;
  workspace_id: string;
  kind: SessionKind;
  role: SessionRole;
  /** `null` for graph roots. */
  parent_session_id: string | null;
  /** Equals `id` for roots (ADR-010). */
  root_session_id: string;
  title: SessionTitle;
  state: SessionState;
  terminal_id: string | null;
  agent_provider_id: string | null;
  /**
   * The launch profile this session ran with (§13.4).
   *
   * Kept even after the profile is deleted: the row is history, and the UI
   * falls back to the provider's own name.
   */
  agent_profile_id: string | null;
  created_at: string;
  /** Bumped by the daemon on activity; used for attention duration labels. */
  last_activity_at?: string;
  /**
   * The commit this session started from, for reading back what it changed.
   *
   * Resolved once at creation and kept across a restart: a restart continues
   * the same unit of work. `null` when there was no HEAD to read.
   */
  base_commit?: string | null;
};

export type ProjectGroup = {
  id: string;
  name: string;
};

export type Project = {
  id: string;
  project_group_id: string | null;
  name: string;
  icon: string | null;
  root_path: string;
};

export type WorkspaceStatus = {
  dirty: boolean;
  ahead: number | null;
  behind: number | null;
  measured_at: string | null;
};

export type Workspace = {
  id: string;
  project_id: string;
  kind: string;
  path: string;
  branch: string | null;
  display_name: string | null;
  managed_by_app: boolean;
  status: WorkspaceStatus;
};

/**
 * How a version probe resolved — `domain::DetectionStatus` on the wire.
 *
 * Serde's externally-tagged form: the unit variants arrive as bare strings and
 * the struct variants as a one-key object.
 */
export type DetectionStatus =
  | "NotFound"
  | "ProbeTimeout"
  | { Installed: { executable: string; version: string | null } }
  | { Rejected: { candidate: string; reason: string } };

export type ProviderInfo = {
  descriptor?: {
    id: string;
    display_name?: string;
    /** Binary names tried in order — the first is what the row shows as its command. */
    binary_candidates?: string[];
    default_args?: string[];
    capabilities?: { supports_initial_prompt?: boolean; supports_review?: boolean };
    prompt?: unknown;
    /** The provider's own read-only posture (§16.9): its flags and what it
     *  calls the mode. Absent means it has none, so it cannot be sent to
     *  review a pull request. */
    review?: { args: string[]; label: string } | null;
    /** How this provider is pointed at a profile's own config directory
     *  (§13.4). Absent means it has no such switch, so a profile for it can
     *  only change the binary and the arguments. */
    config_dir?: ConfigDirSpec | null;
  };
  detection?: { status?: DetectionStatus; checked_at?: string };
};

/** `domain::ConfigDirSpec`: the variables one profile directory arrives as. */
export type ConfigDirSpec = {
  vars: string[];
  help: string;
};

/** The provider's id, which is what every other table keys on. */
export function providerId(provider: ProviderInfo): string {
  return provider.descriptor?.id ?? "";
}

export function providerName(provider: ProviderInfo): string {
  return provider.descriptor?.display_name ?? providerId(provider);
}

export function providerInstalled(provider: ProviderInfo): boolean {
  const status = provider.detection?.status;
  return typeof status === "object" && status !== null && "Installed" in status;
}

/**
 * Whether this provider can be sent to review a pull request (§16.9).
 *
 * Both halves are required and neither implies the other: the launch carries a
 * prompt *and* asks for a read-only mode, and the daemon refuses the whole
 * request when either spelling is missing. Asking here is what keeps the menu
 * from offering a launch that would be turned down.
 */
export function providerReviews(provider: ProviderInfo): boolean {
  return (
    providerInstalled(provider) &&
    provider.descriptor?.review != null &&
    (provider.descriptor?.capabilities?.supports_initial_prompt === true ||
      provider.descriptor?.prompt != null)
  );
}

/** What the provider calls its read-only mode, for a menu that has to say. */
export function providerReviewMode(provider: ProviderInfo): string | null {
  return provider.descriptor?.review?.label ?? null;
}

/**
 * The binary this provider would actually run, and where the answer came from.
 *
 * Detection's `executable` is the resolved path and the only authoritative
 * answer; the first binary candidate is what it *would* look for when nothing
 * has been found yet. Naming which of the two is on screen is the point — a
 * field that shows a name cannot otherwise be told apart from one showing a
 * path that was verified.
 */
export function providerExecutable(
  provider: ProviderInfo,
): { path: string; resolved: boolean } | null {
  const status = provider.detection?.status;
  if (typeof status === "object" && status !== null && "Installed" in status) {
    return { path: status.Installed.executable, resolved: true };
  }
  const candidate = provider.descriptor?.binary_candidates?.[0];
  return candidate ? { path: candidate, resolved: false } : null;
}

/** One line saying what detection found, for the row's badge. */
export function providerDetectionLabel(provider: ProviderInfo): string {
  const status = provider.detection?.status;
  if (status === undefined) return "not probed";
  if (status === "NotFound") return "not found";
  if (status === "ProbeTimeout") return "probe timed out";
  if ("Installed" in status) {
    return status.Installed.version ? `v${status.Installed.version}` : "installed";
  }
  return `rejected: ${status.Rejected.reason}`;
}

export type UsageWindow = {
  used_percent: number;
  window: string;
  resets_at: string | null;
};

export type ProviderUsage = {
  provider_id: string;
  /**
   * The launch profile whose account this reading came from, `null` for the
   * provider's default one (§13.4).
   *
   * One provider can report several: a profile that moved the config directory
   * is a second login with an allowance of its own.
   */
  profile_id: string | null;
  windows: UsageWindow[];
  collected_at: string;
};

export type DaemonInfo = {
  protocol_version: number;
  daemon_version: string;
  instance_id: string;
  started_at: string;
};

export type Launchable = {
  kind: "shell" | "agent";
  label: string;
  detail: string | null;
  provider: string | null;
  profile: string | null;
  enabled: boolean;
  key: string;
  /**
   * Whether this row can start with a prompt already in hand (§16.8).
   *
   * The prompt is one trailing argv entry, so a provider that declares no
   * prompt style refuses the launch outright. The handoff dialog says so up
   * front rather than failing on confirm.
   */
  supports_initial_prompt: boolean;
};

export type AgentProfile = {
  id: string;
  provider_id: string;
  name: string;
  executable: string | null;
  config_dir: string | null;
  args: string[];
  created_at: string;
};

export type JobState = "Queued" | "Running" | "Succeeded" | "Failed" | "Cancelled" | string;

export type Job = {
  id: string;
  provider_id: string;
  workspace_id: string;
  role: string;
  feature_id: number | null;
  parent_session_id: string | null;
  state: JobState;
  summary: string;
  prompt: string;
  provider_session_id: string | null;
  exit_code: number | null;
  last_line: string | null;
  started_at: string;
  finished_at: string | null;
  log_path: string;
};

export type SessionAttentionFlags = {
  wants_you: boolean;
  unread: boolean;
};

/** An agent run found on disk that the daemon did not launch (§13.5). */
export type ExternalAgentSession = {
  /** The provider's own id — and the one its CLI resumes by. */
  session_id: string;
  project_id: string;
  workspace_id: string | null;
  provider: string;
  /**
   * The profile whose account holds this transcript, `null` for the default
   * one (§13.4). Resuming has to pass it back: the run only exists for a CLI
   * started with that profile's config directory.
   */
  profile_id: string | null;
  title: string;
  branch: string | null;
  preview: string | null;
  model: string | null;
  /** Prompts plus replies; tool traffic is not counted. */
  message_count: number;
  subagent_count: number;
  transcript_path: string;
  /**
   * Whether `transcript_path` is this run's alone. A `SharedDatabase` is
   * opencode's whole history, so `Delete` is greyed rather than refused on
   * confirm.
   */
  store: TranscriptStore;
  started_at: string;
  last_activity: string;
};

export type TranscriptStore = "File" | "SharedDatabase";

/**
 * A discovered run's conversation, folded to plain text.
 *
 * `SessionTranscript`'s on-disk twin: a transcript file has no terminal rows,
 * so it counts `turns`. `asSessionTranscript` maps the two.
 */
export type ExternalTranscript = {
  session_id: string;
  text: string;
  turns: number;
  truncated: boolean;
};

export type ReviewDecision = "Approved" | "ChangesRequested" | "ReviewRequired" | string;

export type PullRequest = {
  project_id: string | null;
  repository: string;
  host: string;
  number: number;
  title: string;
  body: string;
  body_truncated: boolean;
  url: string;
  author: string;
  base_ref: string;
  head_ref: string;
  is_draft: boolean;
  review_decision: ReviewDecision | null;
  labels: { name: string; color?: string | null }[];
  assignees: string[];
  review_requests: string[];
  additions: number;
  deletions: number;
  changed_files: number;
  comment_count: number;
  created_at: string;
  updated_at: string;
  /** How the authenticated viewer relates to this pull request. The daemon
   *  asked the host all three questions in the one query it already ran, so
   *  filtering by them costs nothing here (§16.6). */
  relations: PullRequestRelations;
};

export type PullRequestRelations = {
  assigned: boolean;
  review_requested: boolean;
  authored: boolean;
};

/** What went wrong, classified so the panel can write its own sentence. The
 *  daemon knows *which* failure happened; only the GUI knows how to say it to a
 *  person, so `message` is diagnostic detail and never the headline. */
export type PullRequestFailureKind =
  | "CliMissing"
  | "NotSignedIn"
  | "TimedOut"
  | "HostRefused"
  | "RepositoryRefused"
  | "Unknown";

export type PullRequestFailure = {
  host?: string | null;
  repository?: string | null;
  kind: PullRequestFailureKind;
  /** The CLI's own words. For a details disclosure and logs, not the banner. */
  message: string;
};

export type PullRequestState = {
  pull_requests: PullRequest[];
  viewers: { host: string; login: string }[];
  sources: { project_id: string; repository?: string | null }[];
  failures: PullRequestFailure[];
  error: string | null;
  refreshed_at: string | null;
};

/** How one path reaches every workspace of a project (§14.2). */
export type ShareStrategy =
  | { kind: "copy" }
  | { kind: "clone" }
  | { kind: "link" }
  | { kind: "run"; command: string; timeout_secs: number };

export type ShareRule = {
  id: string;
  project_id: string;
  /** Relative to the repository root. */
  path: string;
  strategy: ShareStrategy;
  enabled: boolean;
  position: number;
  created_at: string;
};

export type ShareClass =
  | "Secret"
  | "Dependencies"
  | "BuildOutput"
  | "Cache"
  | "EditorState"
  | "Other";

export type ShareCandidate = {
  path: string;
  class: ShareClass;
  is_dir: boolean;
  size_bytes: number | null;
  entries: number | null;
  suggested: ShareStrategy;
  already_ruled: boolean;
};

export type ShareState =
  | { state: "applied" }
  | { state: "missing" }
  | { state: "severed" }
  | { state: "diverged" }
  | { state: "skipped"; reason: string }
  | { state: "failed"; message: string };

export type ShareStatusEntry = {
  rule_id: string;
  path: string;
  state: ShareState;
};

export type ShareVerb = "copy" | "clone" | "link" | "run" | "skip" | "backup" | "remove";

export type ShareAction = {
  rule_id: string;
  path: string;
  verb: ShareVerb;
  bytes: number | null;
  /** A clone this filesystem could not do, so it was copied. */
  fallback: boolean;
  note: string | null;
};

/** What happens to the files a rule already wrote when the rule is removed. */
export type ShareCleanup = "leave" | "remove_injected" | "materialize";

export type ShareTrigger = "Created" | "Adopted" | "Requested";

export type ShellSnapshot = {
  project_groups: ProjectGroup[];
  projects: Project[];
  workspaces: Workspace[];
  sessions: Session[];
  providers: ProviderInfo[];
  /** Subscription meters from provider endpoints (§16.2). */
  usage: ProviderUsage[];
  external_agents: ExternalAgentSession[];
  pull_requests: PullRequestState;
  launchables: Launchable[];
  /**
   * Launch profiles (§13.4), for the settings section that edits them.
   *
   * `launchables` already flattens these into rows the `+` menu can start, but
   * a profile is also a thing with an executable, arguments and an environment,
   * and none of that survives the flattening.
   */
  agent_profiles: AgentProfile[];
  /**
   * Every project's file-sharing rules (§14.2), in application order.
   *
   * Small rows the settings section needs without a round trip, exactly like
   * `agent_profiles`.
   */
  worktree_shares: ShareRule[];
  session_attention: Record<string, SessionAttentionFlags>;
  /**
   * Persisted GUI preferences (§15.2). The daemon stays authoritative: writes
   * go through `setAppState`, and this is the last read of them.
   */
  app_state: Record<string, string>;
  live_sessions: number;
  installed_agents: number;
  provider_count: number;
};

export type ConnectedPayload = {
  daemon: DaemonInfo;
  session_count: number;
  store: ShellSnapshot;
  active_session: string | null;
  active_terminal: string | null;
};

/**
 * The shell lists, published whenever one of them changes.
 *
 * Terminal output does not travel here: it has its own `runtime:cells` event,
 * so a frame of PTY output never drags the session tree through the IPC
 * boundary with it.
 */
export type StatePayload = {
  store: ShellSnapshot;
  active_session: string | null;
  active_terminal: string | null;
};

export type DisconnectedPayload = {
  reason: string;
};

export type HostStatus = {
  client_ready: boolean;
  connected: boolean;
  session_count: number;
  daemon_version: string | null;
  instance_id: string | null;
  reason: string | null;
};

/** `config_paths` — the host's version and the files it reads (§15.1, §15.4). */
export type ConfigPaths = {
  app_version: string;
  config_file: string | null;
  config_dir: string | null;
  config_exists: boolean;
  logs_dir: string | null;
};

/**
 * What removing a project does to its sessions and its worktrees (§10.2).
 *
 * The host's own vocabulary, not the daemon's: `ProjectRemovalPolicy` in
 * `src-tauri/src/runtime/commands.rs` is what maps these three onto the wire
 * enum, which has longer names and is `#[non_exhaustive]`.
 *
 * No policy deletes a branch. `keep_everything` is refused while the project
 * has running sessions.
 */
export type ProjectRemovalPolicy =
  | "keep_everything"
  | "kill_sessions"
  | "kill_sessions_and_worktrees";

/** Empty or whitespace is unset: `??` would otherwise keep a blank rail row. */
function presentTitle(value: string | null | undefined): string | null {
  if (value == null || value.trim() === "") return null;
  return value;
}

export function sessionTitle(session: Session): string {
  return (
    presentTitle(session.title.user) ??
    presentTitle(session.title.terminal) ??
    fallbackKind(session)
  );
}

export function sessionIsActive(state: SessionState): boolean {
  return state === "Starting" || state === "Running";
}

export function sessionStateLabel(state: SessionState): string {
  if (state === "Starting") return "starting";
  if (state === "Running") return "running";
  if (state === "Orphaned") return "orphaned";
  if (typeof state === "object" && state !== null && "Failed" in state) {
    return state.Failed?.reason ?? "failed";
  }
  if (typeof state === "object" && state !== null && "Exited" in state) {
    const { code, signal } = state.Exited ?? {};
    if (code === 0) return "finished";
    if (code != null) return `exited ${code}`;
    if (signal != null) return `signal ${signal}`;
    return "exited";
  }
  return String(state);
}

export function sessionIsAgent(session: Session): boolean {
  return session.kind === "Agent" || session.agent_provider_id != null;
}

function fallbackKind(session: Session): string {
  if (sessionIsAgent(session)) {
    return session.agent_provider_id ?? "Agent";
  }
  return "Shell";
}

/** Whether this session is a graph root (`root_session_id === id`). */
export function sessionIsRoot(session: Session): boolean {
  return session.parent_session_id === null && session.root_session_id === session.id;
}

export function sessionIsOrchestrator(session: Session): boolean {
  return session.role === "Orchestrator";
}

/**
 * Short label for a harness role, or `null` when it is the generic one.
 *
 * `Generic` gets nothing: prefixing every plain shell with "Generic" would say
 * nothing about six rows out of seven.
 */
export function rolePrefix(role: SessionRole): string | null {
  if (typeof role === "object") return "Agent";
  switch (role) {
    case "Generic":
      return null;
    case "Orchestrator":
    case "Planner":
    case "Researcher":
    case "Executor":
    case "Reviewer":
    case "Tester":
      return role;
    default:
      return "Agent";
  }
}

/** A job that has stopped, whatever the outcome. */
export function jobIsFinal(state: JobState): boolean {
  return state === "Succeeded" || state === "Failed" || state === "Cancelled";
}

export function jobIsRunning(state: JobState): boolean {
  return state === "Queued" || state === "Running";
}

/**
 * What a pending release costs, decided by the host from the release notes'
 * `protocol:` header (`src-tauri/src/updates.rs`).
 *
 * `soft` relaunches the GUI only: the daemon is a separate process, so live
 * sessions and their scrollback survive. `hard` cannot reuse the running
 * daemon, and this build refuses to apply one in place.
 */
export type UpdateKind = "soft" | "hard";

export type UpdateInfo = {
  version: string;
  current_version: string;
  notes: string;
  kind: UpdateKind;
};

/** The `shell:update` event payload — one state, never a partial one. */
export type UpdateState =
  | { state: "checking" }
  | { state: "uptodate" }
  | ({ state: "available" } & UpdateInfo)
  | { state: "downloading"; percent: number }
  | { state: "installing" }
  | { state: "failed"; message: string };
