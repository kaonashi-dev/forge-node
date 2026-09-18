import { createStore } from "solid-js/store";
import type { SessionTranscript } from "../../contracts/workbench";

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
  /** Account to preserve when resuming an external run. */
  profile?: string | null;
  /** `null` while the capture is in flight. */
  transcript: SessionTranscript | null;
  error: string | null;
};

export type SpawnChildRequest = {
  parent: string;
  title: string;
};

export type SendContextRequest = {
  source: string;
  title: string;
};

export const [sessionDialogsStore, setSessionDialogsStore] = createStore({
  /** Opens before the daemon capture arrives, providing immediate feedback. */
  handoff: null as HandoffRequest | null,
  spawnChild: null as SpawnChildRequest | null,
  sendContext: null as SendContextRequest | null,
});

/** Open the handoff dialog for a session; the capture follows. */
export function requestHandoff(request: Omit<HandoffRequest, "transcript" | "error">): void {
  setSessionDialogsStore("handoff", { ...request, transcript: null, error: null });
}

/** The capture landed. Ignored when it is not the session on screen. */
export function applyTranscript(session: string, transcript: SessionTranscript): void {
  if (sessionDialogsStore.handoff?.session !== session) return;
  setSessionDialogsStore("handoff", "transcript", transcript);
}

export function failTranscript(session: string, error: string): void {
  if (sessionDialogsStore.handoff?.session !== session) return;
  setSessionDialogsStore("handoff", "error", error);
}

/** Open the spawn-child dialog for the active session. */
export function requestSpawnChild(request: SpawnChildRequest): void {
  setSessionDialogsStore("spawnChild", request);
}

/** Open the send-context dialog for the active session. */
export function requestSendContext(request: SendContextRequest): void {
  setSessionDialogsStore("sendContext", request);
}
