import { batch } from "solid-js";
import { createStore } from "solid-js/store";
import type { AgentLaunchResult, HandoffProgress } from "../../contracts/workbench";
import type {
  closeSession,
  launchBackgroundAgent,
  loadHandoffProgress,
  selectSession,
} from "./commands";
import {
  continueFromSummaryPrompt,
  extractHandoffSummary,
  type HandoffSource,
} from "./handoffPrompt";

export const MAX_HANDOFF_SUMMARY_LENGTH = 16_000;

type HandoffOptions = Omit<HandoffSource, "transcript" | "truncated"> & {
  returnSession: string | null;
  workspace: string;
  summarizerProvider: string;
  summarizerProfile: string | null;
  targetProvider: string;
  targetProfile: string | null;
  parent: string | null;
  focus: string | null;
};

export type HandoffJob = HandoffOptions & {
  id: string;
  summarizerSession: string | null;
  targetSession: string | null;
  launch: { requestId: string; kind: "summarizer" | "destination" } | null;
  readRequest: string | null;
  progressText: string | null;
  progressTruncated: boolean;
  summary: string;
  summaryEdited: boolean;
  progressOpen: boolean;
  finishing: boolean;
  cancelled: boolean;
  uncertain: boolean;
  error: string | null;
  readError: string | null;
};

type HandoffEffects = {
  closeSession: typeof closeSession;
  launchBackgroundAgent: typeof launchBackgroundAgent;
  loadHandoffProgress: typeof loadHandoffProgress;
  selectSession: typeof selectSession;
  showSession: () => void;
  focusWorkspace: (workspace: string) => void;
  sessionReady: (session: string) => boolean;
};

export function createHandoffJobStore(effects: HandoffEffects) {
  const {
    closeSession,
    launchBackgroundAgent,
    loadHandoffProgress,
    selectSession,
    showSession,
    focusWorkspace,
    sessionReady,
  } = effects;
  const [store, setStore] = createStore<{ job: HandoffJob | null }>({ job: null });
  let completing: string | null = null;

  function handoffJob(): HandoffJob | null {
    return store.job;
  }

  function beginHandoffJob(options: HandoffOptions, prompt: string, readOnly: boolean): boolean {
    if (store.job) {
      setHandoffProgressOpen(true);
      return false;
    }
    const id = crypto.randomUUID();
    const requestId = crypto.randomUUID();
    setStore("job", {
      ...options,
      id,
      summarizerSession: null,
      targetSession: null,
      launch: { requestId, kind: "summarizer" },
      readRequest: null,
      progressText: null,
      progressTruncated: false,
      summary: "",
      summaryEdited: false,
      progressOpen: options.returnSession === null,
      finishing: false,
      cancelled: false,
      uncertain: false,
      error: null,
      readError: null,
    });
    void launchBackgroundAgent({
      request_id: requestId,
      workspace: options.workspace,
      provider: options.summarizerProvider,
      profile: options.summarizerProfile,
      prompt,
      parent: options.parent,
      read_only: readOnly,
    }).catch((error: unknown) => launchRejected(requestId, error));
    return true;
  }

  function errorText(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function launchRejected(requestId: string, error: unknown): void {
    applyHandoffLaunch({
      request_id: requestId,
      session: null,
      error: errorText(error),
      uncertain: false,
    });
  }

  function setHandoffProgressOpen(open: boolean): void {
    if (store.job) setStore("job", "progressOpen", open);
  }

  function setHandoffSummary(summary: string): void {
    if (!store.job || store.job.finishing) return;
    setStore("job", { summary, summaryEdited: true });
  }

  function applyHandoffLaunch(result: AgentLaunchResult): void {
    const job = store.job;
    if (!job?.launch || job.launch.requestId !== result.request_id) return;
    const kind = job.launch.kind;
    setStore("job", "launch", null);
    if (!result.session) {
      if (job.cancelled && !result.uncertain) {
        setStore("job", null);
        return;
      }
      setStore("job", {
        finishing: false,
        uncertain: result.uncertain,
        error: result.uncertain
          ? `The launch outcome is unknown. Check the session list before starting another handoff. ${result.error ?? ""}`
          : (result.error ?? "Could not start the agent."),
        progressOpen: true,
      });
      return;
    }
    if (kind === "summarizer") {
      setStore("job", "summarizerSession", result.session);
      if (job.cancelled) void closeSummarizer(job.id);
      else refreshHandoffProgress();
    } else {
      setStore("job", { targetSession: result.session, finishing: false });
      void completeHandoff();
    }
  }

  async function openHandoffSession(session: string): Promise<boolean> {
    const job = store.job;
    if (!job) return false;
    const id = job.id;
    try {
      if (!sessionReady(session)) throw new Error("The session is not ready to open yet.");
      await selectSession(session);
      if (store.job?.id !== id) return false;
      focusWorkspace(job.workspace);
      showSession();
      return true;
    } catch (error: unknown) {
      if (store.job?.id === id) {
        setStore("job", { finishing: false, error: errorText(error), progressOpen: true });
      }
      return false;
    }
  }

  async function completeHandoff(): Promise<void> {
    const job = store.job;
    if (!job?.targetSession || completing === job.id || !sessionReady(job.targetSession)) return;
    const id = job.id;
    completing = id;
    setStore("job", { finishing: true, error: null });
    try {
      if (await openHandoffSession(job.targetSession)) await closeSummarizer(id);
    } finally {
      if (completing === id) completing = null;
    }
  }

  function syncHandoffSession(): void {
    // Creation replies and runtime snapshots travel on different host threads.
    if (store.job?.targetSession && !store.job.error) void completeHandoff();
  }

  function applyHandoffProgress(result: HandoffProgress): void {
    const job = store.job;
    if (!job || job.readRequest !== result.request_id) return;
    batch(() => {
      setStore("job", "readRequest", null);
      const transcript = result.transcript;
      if (!transcript || transcript.session_id !== job.summarizerSession) {
        setStore("job", {
          readError: result.error ?? "Could not read the summarizer.",
          progressOpen: true,
        });
        return;
      }
      setStore("job", {
        progressText: transcript.text,
        progressTruncated: transcript.truncated,
        readError: null,
      });
      if (job.summaryEdited || job.finishing || job.cancelled) return;
      const summary = extractHandoffSummary(transcript.text);
      if (summary && summary.length <= MAX_HANDOFF_SUMMARY_LENGTH) {
        // A terminal capture has no assistant-turn boundary; only the person can confirm finality.
        setStore("job", { summary, progressOpen: job.progressOpen || job.summary === "" });
      }
    });
  }

  function refreshHandoffProgress(): void {
    const job = store.job;
    if (
      !job?.summarizerSession ||
      job.readRequest ||
      job.readError ||
      job.finishing ||
      job.cancelled ||
      job.targetSession
    )
      return;
    const requestId = crypto.randomUUID();
    setStore("job", "readRequest", requestId);
    void loadHandoffProgress(requestId, job.summarizerSession).catch((error: unknown) => {
      applyHandoffProgress({ request_id: requestId, transcript: null, error: errorText(error) });
    });
  }

  function retryHandoffProgress(): void {
    if (!store.job) return;
    setStore("job", "readError", null);
    refreshHandoffProgress();
  }

  async function finishHandoffWithSummary(summary: string): Promise<void> {
    const job = store.job;
    if (
      !job?.summarizerSession ||
      job.launch ||
      job.finishing ||
      job.cancelled ||
      job.uncertain ||
      job.targetSession
    )
      return;
    const prompt =
      summary.length <= MAX_HANDOFF_SUMMARY_LENGTH
        ? continueFromSummaryPrompt(job, summary, job.focus)
        : null;
    if (!prompt) {
      setStore("job", {
        error: "Enter a non-empty brief of at most 16,000 characters.",
        progressOpen: true,
      });
      return;
    }
    const requestId = crypto.randomUUID();
    setStore("job", {
      summary,
      summaryEdited: true,
      finishing: true,
      error: null,
      launch: { requestId, kind: "destination" },
    });
    try {
      await launchBackgroundAgent({
        request_id: requestId,
        workspace: job.workspace,
        provider: job.targetProvider,
        profile: job.targetProfile,
        prompt,
        parent: job.parent,
        read_only: false,
      });
    } catch (error: unknown) {
      launchRejected(requestId, error);
    }
  }

  async function closeSummarizer(id: string): Promise<void> {
    const job = store.job;
    if (!job || job.id !== id) return;
    try {
      if (job.summarizerSession) await closeSession(job.summarizerSession);
      if (store.job?.id === id) setStore("job", null);
    } catch (error: unknown) {
      if (store.job?.id === id) {
        setStore("job", {
          finishing: false,
          error: `Could not close the summarizer: ${errorText(error)}`,
          progressOpen: true,
        });
      }
    }
  }

  function cancelHandoffJob(): void {
    const job = store.job;
    if (!job || job.finishing) return;
    setStore("job", { cancelled: true, progressOpen: true });
    // Keep ownership until a queued launch answers so cancellation can close that exact session.
    if (!job.launch) void closeSummarizer(job.id);
  }

  return {
    handoffJob,
    beginHandoffJob,
    setHandoffProgressOpen,
    setHandoffSummary,
    applyHandoffLaunch,
    openHandoffSession,
    completeHandoff,
    syncHandoffSession,
    applyHandoffProgress,
    refreshHandoffProgress,
    retryHandoffProgress,
    finishHandoffWithSummary,
    cancelHandoffJob,
  };
}
