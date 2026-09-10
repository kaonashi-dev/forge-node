import { createStore } from "solid-js/store";
import type { SessionTranscript } from "../workbench/types";

export type ConnectionState =
  | { kind: "idle" }
  | { kind: "connecting" }
  | { kind: "connected"; instanceId: string; version: string }
  | { kind: "disconnected"; reason: string };

export type WorktreeRemoval = {
  workspace: string;
  /** `null` is the initial confirmation; a reason asks permission to force. */
  reason: string | null;
};

export type WorktreeCreationFailure = {
  project: string;
  branch: string;
  reason: string;
};

export type ProfileSaveResult = {
  profile: string;
  error: string | null;
};

/**
 * A destructive choice waiting for an answer.
 *
 * `window.confirm` was what asked before this existed, and it fails on all
 * four counts the rest of the shell is held to: it cannot be styled, it cannot
 * be reached by the ARIA the rest of the dialogs carry, it blocks the whole
 * WebView while it is up — including the event loop draining the daemon's
 * channel — and no test can answer it. One request lives here and `AppShell`
 * renders it as an `AlertDialog`, so the four call sites keep their one-line
 * shape and gain the `alertdialog` role.
 */
export type TextInputRequest = {
  title: string;
  label: string;
  value: string;
  placeholder?: string;
  confirmLabel: string;
  allowEmpty: boolean;
  onSubmit: (value: string) => void;
};

export type ConfirmRequest = {
  title: string;
  /** The consequence, in a sentence. Skipped when the title says it all. */
  description?: string;
  /** Label on the button that goes ahead. Names the act: "Close", "Abort". */
  confirmLabel: string;
  /** Draws the confirm button in danger colours; true for anything that loses work. */
  destructive?: boolean;
  onConfirm: () => void;
};

export const [runtimeStore, setRuntimeStore] = createStore({
  connection: { kind: "idle" } as ConnectionState,
  activeSession: null as string | null,
  activeTerminal: null as string | null,
  /**
   * The last thing the host refused to do.
   *
   * Separate from `connection` on purpose: a refused command — clicking a
   * session that already exited — is not a lost connection, and showing it on
   * the daemon pill would say the wrong thing about both.
   */
  notice: null as string | null,
  worktreeRemoval: null as WorktreeRemoval | null,
  worktreeCreationFailure: null as WorktreeCreationFailure | null,
  profileSave: null as ProfileSaveResult | null,
  /**
   * The project the new-worktree dialog is asking about.
   *
   * Window state rather than a local signal on AppShell: the rail's context
   * menu must name the row it was opened from, which is not always the active
   * session's project, and the dialog must survive the sidebar unmounting.
   */
  branchPicker: null as { id: string; name: string } | null,
  /**
   * The project the removal dialog is asking about.
   *
   * Window state rather than the rail's own: the rail unmounts with the
   * sidebar, and a dialog that disappears because the panel behind it was
   * collapsed is a dialog that answered itself.
   */
  projectRemoval: null as string | null,
  confirm: null as ConfirmRequest | null,
  /**
   * A name being asked for.
   *
   * Beside `confirm`, and for the same reason: `AppShell` used to hold this in
   * a local signal and pass a setter down through `Sidebar`, which meant only
   * the components on that prop path could ask a question. The file tree needs
   * it too (§2.2 A11) and is nowhere near it.
   */
  textInput: null as TextInputRequest | null,
  /**
   * The handoff dialog's session, and the capture it is waiting on (§16.8).
   *
   * The capture is a round trip to the daemon, so the dialog opens first and
   * fills in: asking someone to wait on a spinner with no dialog around it
   * reads as a click that did nothing.
   */
  handoff: null as HandoffRequest | null,
});

export type HandoffRequest = {
  /**
   * Which kind of run this came from. A live session's capture is its
   * terminal; a discovered one's is a transcript file, and the two arrive on
   * different events into this one slot.
   */
  kind: "session" | "external";
  /** Correlation key: the daemon session id, or the provider's own id. */
  session: string;
  /** Resolved title of the source session, as the tab strip shows it. */
  title: string;
  workspace: string;
  workingDirectory: string;
  branch: string | null;
  sourceAgent: string | null;
  /** The account a `kind: "external"` run was found in (§13.4). */
  profile?: string | null;
  /** `null` while the capture is in flight. */
  transcript: SessionTranscript | null;
  error: string | null;
};

/** Open the handoff dialog for a session; the capture follows. */
export function requestHandoff(request: Omit<HandoffRequest, "transcript" | "error">): void {
  setRuntimeStore("handoff", { ...request, transcript: null, error: null });
}

/** The capture landed. Ignored when it is not the session on screen. */
export function applyTranscript(session: string, transcript: SessionTranscript): void {
  if (runtimeStore.handoff?.session !== session) return;
  setRuntimeStore("handoff", "transcript", transcript);
}

export function failTranscript(session: string, error: string): void {
  if (runtimeStore.handoff?.session !== session) return;
  setRuntimeStore("handoff", "error", error);
}

/** Ask for a name. Replaces `window.prompt`, which WKWebView does not show. */
export function requestTextInput(request: TextInputRequest): void {
  setRuntimeStore("textInput", request);
}

/** Open the new-worktree dialog for the project the caller named. */
export function requestNewWorktree(project: { id: string; name: string }): void {
  setRuntimeStore("branchPicker", project);
}

/** Put a destructive choice to the person. Replaces `window.confirm`. */
export function requestConfirm(request: ConfirmRequest): void {
  setRuntimeStore("confirm", request);
}
