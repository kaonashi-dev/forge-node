import { invoke } from "@tauri-apps/api/core";
import type {
  ConfigPaths,
  ConnectedPayload,
  HostStatus,
  ProjectRemovalPolicy,
  ShareCleanup,
  ShareRule,
  UpdateInfo,
  WorktreeIgnore,
} from "./types";

export async function hostStatus(): Promise<HostStatus> {
  return invoke<HostStatus>("host_status");
}

export async function connect(): Promise<ConnectedPayload | null> {
  return invoke<ConnectedPayload | null>("connect");
}

export async function reconnect(): Promise<void> {
  await invoke("reconnect");
}

/**
 * Where Forge reads its configuration from (§15.4).
 *
 * Asked for on demand rather than carried on the snapshot: it is host state,
 * not daemon state, and only the settings screen ever wants it.
 */
export async function configPaths(): Promise<ConfigPaths> {
  return invoke<ConfigPaths>("config_paths");
}

/**
 * Send one command to the runtime thread.
 *
 * Failures are swallowed by the callers below on purpose: the channel is
 * bounded and the host reports what actually happened through `runtime:*`
 * events, so a rejected promise here would be a second, contradictory source
 * of truth about the connection.
 */
async function send(command: Record<string, unknown>): Promise<void> {
  await invoke("send_runtime_command", { command });
}

export async function selectSession(sessionId: string): Promise<void> {
  await send({ type: "select_session", session_id: sessionId });
}

export async function newShell(workspace: string | null = null): Promise<void> {
  await send({ type: "new_shell", workspace });
}

export async function closeSession(sessionId: string): Promise<void> {
  await send({ type: "close_session", session_id: sessionId });
}

/** Stop a running session without removing its row (§7.3). */
export async function killSession(sessionId: string): Promise<void> {
  await send({ type: "kill_session", session_id: sessionId });
}

/** Bring an exited or orphaned session back with a fresh PTY (§7.3). */
export async function restartSession(sessionId: string): Promise<void> {
  await send({ type: "restart_session", session_id: sessionId });
}

/** `null` clears the user title so the terminal-reported one takes over again. */
export async function renameSession(sessionId: string, title: string | null): Promise<void> {
  await send({ type: "rename_session", session_id: sessionId, title });
}

export async function refreshWorkspaceStatus(workspace: string): Promise<void> {
  await send({ type: "refresh_workspace_status", workspace });
}

export async function createWorktree(
  project: string,
  branch: string,
  base: string | null = null,
  name: string | null = null,
): Promise<void> {
  await send({ type: "create_worktree", project, branch, base, name });
}

export async function fetchRemote(project: string, remote: string | null = null): Promise<void> {
  await send({ type: "fetch_remote", project, remote });
}

export async function openInEditor(editor: string, path: string): Promise<void> {
  await send({ type: "open_in_editor", editor, path });
}

export async function openInFileManager(path: string): Promise<void> {
  await send({ type: "open_in_file_manager", path });
}

export async function openUrl(url: string): Promise<void> {
  await send({ type: "open_url", url });
}

/**
 * Launch an agent, optionally re-entering an existing conversation.
 *
 * `resume` is the provider's own session id (§13.5). The provider's CLI does
 * the re-entering; nothing here replays a transcript.
 *
 * `readOnly` asks for the provider's own read-only mode (§16.9) and tags the
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
  await send({
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

/** Spawn a child agent under a parent session (§8.2). */
export async function createChildSession(args: {
  parent: string;
  provider: string;
  profile?: string | null;
  prompt?: string | null;
  role?: string | null;
  workspacePolicy?: "same" | "worktree";
  branchHint?: string | null;
}): Promise<void> {
  await send({
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

/** Persist a context envelope and deliver it or spawn a child (§8.3). */
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
  await send({
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

/** Read output written before a job viewer was opened. */
export async function readJobLog(jobId: string): Promise<void> {
  await send({ type: "read_job_log", job_id: jobId });
}

export async function refreshSnapshot(): Promise<void> {
  await send({ type: "refresh_snapshot" });
}

/**
 * Persist one GUI preference (§15.2).
 *
 * The daemon owns it: this writes, and the value comes back on the next
 * snapshot rather than the WebView keeping a second copy.
 */
export async function setAppState(key: string, value: string): Promise<void> {
  await send({ type: "set_app_state", key, value });
}

/** Restore Forge-owned state while retaining config, logs, and repository files. */
export async function factoryReset(): Promise<void> {
  await send({ type: "factory_reset" });
}

// --- Projects, groups and checkouts -----------------------------------------
//
// Every one of these acks and then broadcasts: the row that changed redraws
// from the snapshot the daemon sends, never from a return value here.

/**
 * Ask the platform for a directory, then register it.
 *
 * The picker is modal, so it lives on the host and answers on its own channel;
 * `null` means the user cancelled, which is not an error and gets no notice.
 */
export async function addProjectFromPicker(group: string | null = null): Promise<void> {
  const path = await invoke<string | null>("pick_directory", { title: "Add project" });
  if (path === null) return;
  await send({ type: "add_project", path, group });
}

export async function addProject(path: string, group: string | null = null): Promise<void> {
  await send({ type: "add_project", path, group });
}

/** Re-detect a project's git root, branches and worktrees (§10.2). */
export async function refreshProject(project: string): Promise<void> {
  await send({ type: "refresh_project", project });
}

/** `null` moves it to General. */
export async function moveProject(project: string, group: string | null): Promise<void> {
  await send({ type: "move_project", project, group });
}

/** `null` clears it, so the rail falls back to the initials of the name. */
export async function setProjectIcon(project: string, icon: string | null): Promise<void> {
  await send({ type: "set_project_icon", project, icon });
}

export async function createProjectGroup(name: string): Promise<void> {
  await send({ type: "create_project_group", name });
}

export async function renameProjectGroup(group: string, name: string): Promise<void> {
  await send({ type: "rename_project_group", group, name });
}

/** Removes the grouping only; the project directories are untouched. */
export async function removeProjectGroup(group: string): Promise<void> {
  await send({ type: "remove_project_group", group });
}

/**
 * Remove a project (§10.2).
 *
 * No policy ever deletes a branch, and only `kill_sessions_and_worktrees`
 * touches the disk — and then only the worktrees Forge created itself. The
 * daemon refuses `keep_everything` while sessions are still running, which is
 * why the dialog asks for the policy up front rather than discovering it from
 * a refusal.
 */
export async function removeProject(project: string, policy: ProjectRemovalPolicy): Promise<void> {
  await send({ type: "remove_project", project, policy });
}

/** `null` falls back to the branch name. */
export async function renameWorkspace(
  workspace: string,
  displayName: string | null,
): Promise<void> {
  await send({ type: "rename_workspace", workspace, display_name: displayName });
}

/**
 * Remove a managed worktree (§14.4). Never deletes a branch.
 *
 * Without `force` the daemon refuses while the tree is dirty, a merge is in
 * progress, or sessions are running. The host reports that refusal through a
 * contextual event so the shell can ask before reissuing with `force`.
 */
export async function removeWorktree(workspace: string, force = false): Promise<void> {
  await send({ type: "remove_worktree", workspace, force });
}

// --- Agents -----------------------------------------------------------------

/** Re-probe the agent CLIs; `null` refreshes every provider (§13.1). */
export async function refreshDetection(provider: string | null = null): Promise<void> {
  await send({ type: "refresh_detection", provider });
}

/** A launch profile (§13.4): a provider plus the way this user runs it. */
export type AgentProfile = {
  id: string;
  provider_id: string;
  name: string;
  /**
   * Program to run instead of the detected binary; `null` inherits it. A bare
   * name is looked up on the login shell's PATH, which is where the wrapper
   * script behind a shell alias lives.
   */
  executable: string | null;
  /**
   * The account this profile logs into, as the provider's own config-directory
   * variables. Relative to `$HOME` unless absolute; `null` shares the
   * provider's default account.
   */
  config_dir: string | null;
  /** Appended after the descriptor's own default arguments. */
  args: string[];
  created_at: string;
};

export async function saveAgentProfile(profile: AgentProfile): Promise<void> {
  await send({ type: "save_agent_profile", profile });
}

export async function removeAgentProfile(profile: string): Promise<void> {
  await send({ type: "remove_agent_profile", profile });
}

// --- Shared files between worktrees (§14.2) ---------------------------------

/**
 * Replace a project's whole rule set.
 *
 * The set, not a row: adding, reordering, enabling and re-pointing a strategy
 * are one edit of one list, so they are one request and one event. Removal is
 * separate — it is the only edit that can delete a file.
 */
export async function setProjectShares(project: string, rules: ShareRule[]): Promise<void> {
  await send({ type: "set_project_shares", project, rules });
}

/** Drop one rule, saying what happens to the files it already wrote. */
export async function removeShareRule(
  project: string,
  rule: string,
  cleanup: ShareCleanup = "leave",
): Promise<void> {
  await send({ type: "remove_share_rule", project, rule, cleanup });
}

/** Ask the platform for a file to share. `null` means the user cancelled. */
export async function pickFile(title?: string, directory?: string): Promise<string | null> {
  return (await invoke<string | null>("pick_file", { title, directory })) ?? null;
}

// --- Worktrees Forge is told to forget (§14.4) ------------------------------

/**
 * Replace a project's whole ignore set.
 *
 * The set, not a row, for the same reason as the share rules: adding and
 * removing are one edit of one list. The daemon rescans the project before it
 * acks, so the rail has already dropped what the new rules cover when this
 * returns.
 */
export async function setWorktreeIgnores(project: string, rules: WorktreeIgnore[]): Promise<void> {
  await send({ type: "set_worktree_ignores", project, rules });
}

/** Pin the executable used for a provider, or clear the override with `null`. */
export async function setProviderExecutable(provider: string, path: string | null): Promise<void> {
  await send({ type: "set_provider_executable", provider, path });
}

/**
 * Ask the daemon to stop.
 *
 * The connection goes with it, so the shell reconnects afterwards: it finds
 * nothing, says so, and keeps retrying until a daemon is started again.
 */
export async function stopDaemon(killSessions = false): Promise<void> {
  await send({ type: "stop_daemon", kill_sessions: killSessions });
}

// --- Terminal ---------------------------------------------------------------

export type KeyPress = {
  /** `KeyboardEvent.key`. */
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
};

export async function sendKey(key: KeyPress, id: number): Promise<void> {
  await send({ type: "input", key, id });
}

/** Text an input method committed: typed, so never bracketed. */
export async function sendText(text: string, id: number): Promise<void> {
  await send({ type: "input_text", text, id });
}

/**
 * A mouse event for a program that asked to read the mouse (§11.6).
 *
 * Only sent while the terminal is in a reporting mode and `shift` is not held:
 * the pane keeps the mouse for selection otherwise, and `shift` is the xterm
 * convention for taking it back while a program has it.
 */
export async function sendMouse(event: {
  /** `left`, `middle`, `right`, `wheel_up`, `wheel_down`. */
  button: string;
  /** `press`, `release`, `motion`. */
  kind: string;
  /** 0-based cell coordinates. */
  col: number;
  row: number;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
}): Promise<void> {
  await send({ type: "mouse", ...event });
}

export async function sendPaste(text: string, id: number): Promise<void> {
  await send({ type: "paste", text, id });
}

export async function resizeTerminal(
  cols: number,
  rows: number,
  pixelWidth: number,
  pixelHeight: number,
): Promise<void> {
  await send({
    type: "resize",
    size: { cols, rows, pixel_width: pixelWidth, pixel_height: pixelHeight },
  });
}

/** Positive moves into history, negative back towards the live output. */
export async function scrollTerminal(lines: number): Promise<void> {
  await send({ type: "scroll", lines });
}

export async function scrollToBottom(): Promise<void> {
  await send({ type: "scroll_to_bottom" });
}

/**
 * Ask for the whole viewport again.
 *
 * The host connects and attaches before the WebView exists, so the frame that
 * came with the attach had no canvas to reach; the pane asks for one when it
 * mounts rather than waiting for output an idle shell will never produce.
 */
export async function repaintTerminal(): Promise<void> {
  await send({ type: "repaint" });
}

/**
 * Ask the host to cut a dragged range.
 *
 * Only the grid knows what a run's cells hold, so the text comes back on the
 * `runtime:clipboard` event rather than being reconstructed from what the
 * canvas was given to paint.
 */
export async function copySelection(
  anchor: { line: number; col: number },
  head: { line: number; col: number },
): Promise<void> {
  await send({
    type: "copy_selection",
    anchor_line: anchor.line,
    anchor_col: anchor.col,
    head_line: head.line,
    head_col: head.col,
  });
}

/**
 * Force an update check, for the `Check for Updates…` menu item.
 *
 * `null` means "already the newest". The background schedule in the host stays
 * quiet about that; a check the user asked for is the one case worth saying.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("check_for_update");
}

/**
 * Download the pending update, swap the bundle and relaunch.
 *
 * Resolves only on failure: on success the process is replaced. Progress
 * arrives on `shell:update`, not here.
 */
export async function installUpdate(): Promise<void> {
  await invoke("install_update");
}
