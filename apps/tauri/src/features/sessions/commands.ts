import { sendRuntimeCommand } from "../../runtime/host";
import { sendWorkbenchCommand } from "../../runtime/workbench";
import { beginSessionSelection } from "../../state/connection";

export async function selectSession(sessionId: string): Promise<void> {
  const cancel = beginSessionSelection(sessionId);
  try {
    await sendRuntimeCommand({ type: "select_session", session_id: sessionId });
  } catch (error) {
    cancel();
    throw error;
  }
}

export async function newShell(workspace: string | null = null): Promise<void> {
  await sendRuntimeCommand({ type: "new_shell", workspace });
}

/** A new shell that stays beside the current one, rather than replacing it. */
export async function splitShell(workspace: string | null = null): Promise<void> {
  await sendRuntimeCommand({ type: "split_shell", workspace });
}

export async function detachSplit(sessionId: string): Promise<void> {
  await sendRuntimeCommand({ type: "detach_split", session_id: sessionId });
}

/** A hidden column keeps its attachment but stops receiving frames. */
export async function parkSplit(sessionId: string, parked: boolean): Promise<void> {
  await sendRuntimeCommand({ type: "park_split", session_id: sessionId, parked });
}

export async function closeSession(sessionId: string): Promise<void> {
  await sendRuntimeCommand({ type: "close_session", session_id: sessionId });
}

/** Stops the process but retains the session row. */
export async function killSession(sessionId: string): Promise<void> {
  await sendRuntimeCommand({ type: "kill_session", session_id: sessionId });
}

/** Restarts with a fresh PTY. */
export async function restartSession(sessionId: string): Promise<void> {
  await sendRuntimeCommand({ type: "restart_session", session_id: sessionId });
}

/** `null` clears the user title so the terminal-reported one takes over again. */
export async function renameSession(sessionId: string, title: string | null): Promise<void> {
  await sendRuntimeCommand({ type: "rename_session", session_id: sessionId, title });
}

export async function refreshWorkspaceStatus(workspace: string): Promise<void> {
  await sendRuntimeCommand({ type: "refresh_workspace_status", workspace });
}

/**
 * Launch an agent, optionally re-entering an existing conversation.
 *
 * `resume` is the provider's own session id. The provider's CLI does
 * the re-entering; nothing here replays a transcript.
 *
 * `readOnly` asks for the provider's own read-only mode and tags the
 * session as a review. The daemon refuses a provider that declares no such
 * mode, so ask {@link providerReviews} before offering it.
 */
export async function newAgent(
  provider: string,
  profile: string | null = null,
  workspace: string | null = null,
  resume: string | null = null,
  prompt: string | null = null,
  /** The session this was handed off from, recorded as a graph edge (ADR-010). */
  parent: string | null = null,
  readOnly = false,
): Promise<void> {
  await sendRuntimeCommand({
    type: "new_agent",
    provider,
    profile,
    workspace,
    resume,
    prompt,
    parent,
    read_only: readOnly,
  });
}

export async function launchBackgroundAgent(request: {
  request_id: string;
  workspace: string;
  provider: string;
  profile: string | null;
  prompt: string;
  parent: string | null;
  read_only: boolean;
}): Promise<void> {
  await sendWorkbenchCommand({ type: "launch_agent", ...request });
}

export async function loadHandoffProgress(requestId: string, session: string): Promise<void> {
  await sendWorkbenchCommand({ type: "load_handoff_progress", request_id: requestId, session });
}

export async function createChildSession(args: {
  parent: string;
  provider: string;
  profile?: string | null;
  prompt?: string | null;
  role?: string | null;
  workspacePolicy?: "same" | "worktree";
  branchHint?: string | null;
}): Promise<void> {
  await sendRuntimeCommand({
    type: "create_child_session",
    parent: args.parent,
    provider: args.provider,
    profile: args.profile ?? null,
    prompt: args.prompt ?? null,
    role: args.role ?? null,
    workspace_policy: args.workspacePolicy ?? "same",
    branch_hint: args.branchHint ?? null,
  });
}

/** Persists before delivery or child launch. */
export async function sendContext(args: {
  source: string;
  target?: string | null;
  spawnProvider?: string | null;
  profile?: string | null;
  summary?: string | null;
  instructions?: string | null;
  includeTranscript?: boolean;
  role?: string | null;
  workspacePolicy?: "same" | "worktree";
  branchHint?: string | null;
}): Promise<void> {
  await sendRuntimeCommand({
    type: "send_context",
    source: args.source,
    target: args.target ?? null,
    spawn_provider: args.spawnProvider ?? null,
    profile: args.profile ?? null,
    summary: args.summary ?? null,
    instructions: args.instructions ?? null,
    include_transcript: args.includeTranscript ?? false,
    role: args.role ?? null,
    workspace_policy: args.workspacePolicy ?? "same",
    branch_hint: args.branchHint ?? null,
  });
}

/**
 * Terminal tail for a handoff prompt.
 *
 * `null` for either bound takes the daemon's default, which is sized so the
 * prompt built from it still fits in one argv entry.
 */
export async function loadSessionTranscript(
  session: string,
  maxLines: number | null = null,
  maxBytes: number | null = null,
): Promise<void> {
  await sendWorkbenchCommand({
    type: "load_session_transcript",
    session,
    max_lines: maxLines,
    max_bytes: maxBytes,
  });
}

/**
 * Discovered transcript for a handoff prompt.
 *
 * The on-disk twin of `loadSessionTranscript`: the run has no PTY, so the
 * daemon reads its transcript file. Named by identity — a path on the wire
 * would be an arbitrary-file primitive.
 */
export async function loadExternalTranscript(
  session: string,
  provider: string,
  profile: string | null = null,
  maxTurns: number | null = null,
  maxBytes: number | null = null,
): Promise<void> {
  await sendWorkbenchCommand({
    type: "load_external_transcript",
    session,
    provider,
    profile,
    max_turns: maxTurns,
    max_bytes: maxBytes,
  });
}

/** Remove a discovered run's transcript from disk. Never the checkout. */
export async function deleteExternalSession(
  session: string,
  provider: string,
  profile: string | null = null,
): Promise<void> {
  await sendWorkbenchCommand({ type: "delete_external_session", session, provider, profile });
}
