import { beforeEach, describe, expect, it, vi } from "vitest";
import { HANDOFF_MARK_END, HANDOFF_MARK_START, summarizerPrompt } from "./handoffPrompt";
import { createHandoffJobStore } from "./handoffJob";

const commands = {
  launchBackgroundAgent: vi.fn(async (_request: unknown) => {}),
  loadHandoffProgress: vi.fn(async (_request: string, _session: string) => {}),
  closeSession: vi.fn(async (_session: string) => {}),
  selectSession: vi.fn(async (_session: string) => {}),
};

const options = {
  returnSession: "source",
  workspace: "checkout",
  workingDirectory: "/repo",
  branch: "main",
  sourceAgent: "claude",
  sourceTitle: "Auth",
  summarizerProvider: "claude",
  summarizerProfile: "flash-profile",
  targetProvider: "codex",
  targetProfile: "implementation-profile",
  parent: "source",
  focus: "auth",
};

let jobs: ReturnType<typeof createHandoffJobStore>;
const sessionReady = vi.fn((_session: string) => true);

beforeEach(() => {
  for (const command of Object.values(commands)) command.mockReset().mockResolvedValue(undefined);
  sessionReady.mockReset().mockReturnValue(true);
  jobs = createHandoffJobStore({
    ...commands,
    showSession: vi.fn(),
    focusWorkspace: vi.fn(),
    sessionReady,
  });
});

function begin(): string {
  expect(jobs.beginHandoffJob(options, "summarize", true)).toBe(true);
  return jobs.handoffJob()!.launch!.requestId;
}

function launched(requestId: string, session = "summarizer"): void {
  jobs.applyHandoffLaunch({ request_id: requestId, session, error: null, uncertain: false });
}

function capture(text: string): void {
  jobs.applyHandoffProgress({
    request_id: jobs.handoffJob()!.readRequest!,
    transcript: { session_id: "summarizer", text, lines: 1, truncated: false },
    error: null,
  });
}

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

describe("handoff ownership", () => {
  it("adopts only the correlated creation result and never switches away from the source", async () => {
    const requestId = begin();
    await Promise.resolve();
    expect(jobs.handoffJob()?.summarizerSession).toBeNull();
    launched("unrelated-launch", "another-agent");
    expect(jobs.handoffJob()?.summarizerSession).toBeNull();
    launched(requestId);
    expect(jobs.handoffJob()?.summarizerSession).toBe("summarizer");
    expect(commands.selectSession).not.toHaveBeenCalled();
    expect(commands.launchBackgroundAgent).toHaveBeenCalledWith(
      expect.objectContaining({
        provider: "claude",
        profile: "flash-profile",
        read_only: true,
        workspace: "checkout",
      }),
    );
    jobs.cancelHandoffJob();
    expect(commands.closeSession).toHaveBeenCalledTimes(1);
    expect(commands.closeSession).toHaveBeenCalledWith("summarizer");
  });

  it("keeps a cancelled pending launch owned until its exact session can be closed", async () => {
    const requestId = begin();
    jobs.cancelHandoffJob();
    expect(jobs.beginHandoffJob({ ...options, workspace: "other" }, "another", true)).toBe(false);
    expect(commands.closeSession).not.toHaveBeenCalled();
    launched(requestId);
    await vi.waitFor(() => expect(jobs.handoffJob()).toBeNull());
    expect(commands.closeSession).toHaveBeenCalledTimes(1);
    expect(commands.closeSession).toHaveBeenCalledWith("summarizer");
    expect(commands.loadHandoffProgress).not.toHaveBeenCalled();
  });

  it("does not let a stale enqueue failure clear a later handoff", async () => {
    const accepted = deferred();
    commands.launchBackgroundAgent.mockReturnValueOnce(accepted.promise);
    const old = begin();
    launched(old);
    jobs.cancelHandoffJob();
    await vi.waitFor(() => expect(jobs.handoffJob()).toBeNull());
    const current = begin();
    accepted.reject(new Error("late transport callback"));
    await Promise.resolve();
    launched(old, "stale-session");
    expect(jobs.handoffJob()?.launch?.requestId).toBe(current);
    expect(jobs.handoffJob()?.error).toBeNull();
  });

  it("surfaces summarizer failure instead of waiting for a matching snapshot", () => {
    const requestId = begin();
    jobs.applyHandoffLaunch({
      request_id: requestId,
      session: null,
      error: "Profile deleted",
      uncertain: false,
    });
    expect(jobs.handoffJob()).toMatchObject({
      launch: null,
      error: "Profile deleted",
      progressOpen: true,
    });
    expect(commands.loadHandoffProgress).not.toHaveBeenCalled();
    expect(commands.closeSession).not.toHaveBeenCalled();
  });
});

describe("handoff confirmation", () => {
  it("never launches from a prompt echo or a draft, and preserves manual edits", () => {
    launched(begin());
    const prompt = summarizerPrompt(
      { ...options, transcript: "prior work", truncated: false },
      null,
    )!;
    capture(prompt);
    expect(jobs.handoffJob()?.summary).toBe("");
    jobs.refreshHandoffProgress();
    capture(`${HANDOFF_MARK_START}\ndraft\n${HANDOFF_MARK_END}`);
    expect(jobs.handoffJob()?.summary).toBe("draft");
    jobs.setHandoffSummary("Reviewed brief");
    jobs.refreshHandoffProgress();
    capture(`${HANDOFF_MARK_START}\nnewer draft\n${HANDOFF_MARK_END}`);
    expect(jobs.handoffJob()?.summary).toBe("Reviewed brief");
    expect(commands.launchBackgroundAgent).toHaveBeenCalledTimes(1);
    expect(commands.closeSession).not.toHaveBeenCalled();
  });

  it("accepts a manually supplied brief without markers and waits for destination creation", async () => {
    launched(begin());
    capture("A useful answer without delimiters");
    jobs.setHandoffSummary("Finish the auth tests.");
    await jobs.finishHandoffWithSummary(jobs.handoffJob()!.summary);
    expect(jobs.handoffJob()?.finishing).toBe(true);
    expect(commands.closeSession).not.toHaveBeenCalled();
    expect(jobs.beginHandoffJob(options, "overlap", true)).toBe(false);
    await jobs.finishHandoffWithSummary("duplicate");
    expect(commands.launchBackgroundAgent).toHaveBeenCalledTimes(2);
    expect(commands.launchBackgroundAgent).toHaveBeenLastCalledWith(
      expect.objectContaining({
        provider: "codex",
        profile: "implementation-profile",
        parent: "source",
        read_only: false,
        prompt: expect.stringContaining("Finish the auth tests."),
      }),
    );
    launched(jobs.handoffJob()!.launch!.requestId, "destination");
    await vi.waitFor(() => expect(jobs.handoffJob()).toBeNull());
    expect(commands.selectSession).toHaveBeenCalledTimes(1);
    expect(commands.selectSession).toHaveBeenCalledWith("destination");
    expect(commands.closeSession).toHaveBeenCalledTimes(1);
    expect(commands.closeSession).toHaveBeenCalledWith("summarizer");
  });

  it("retains the brief and summarizer when the destination is refused", async () => {
    launched(begin());
    await jobs.finishHandoffWithSummary("Reviewed context");
    const requestId = jobs.handoffJob()!.launch!.requestId;
    jobs.applyHandoffLaunch({
      request_id: requestId,
      session: null,
      error: "Agent not installed",
      uncertain: false,
    });
    expect(jobs.handoffJob()).toMatchObject({
      summary: "Reviewed context",
      finishing: false,
      error: "Agent not installed",
    });
    expect(commands.closeSession).not.toHaveBeenCalled();
    await jobs.finishHandoffWithSummary("Reviewed context");
    expect(commands.launchBackgroundAgent).toHaveBeenCalledTimes(3);
  });

  it("waits for the exact destination in the runtime snapshot before selecting it", async () => {
    launched(begin());
    await jobs.finishHandoffWithSummary("Reviewed context");
    sessionReady.mockReturnValue(false);
    launched(jobs.handoffJob()!.launch!.requestId, "destination");
    expect(commands.selectSession).not.toHaveBeenCalled();
    expect(commands.closeSession).not.toHaveBeenCalled();
    sessionReady.mockImplementation((id) => id === "destination");
    jobs.syncHandoffSession();
    jobs.syncHandoffSession();
    await vi.waitFor(() => expect(jobs.handoffJob()).toBeNull());
    expect(commands.selectSession).toHaveBeenCalledTimes(1);
    expect(commands.selectSession).toHaveBeenCalledWith("destination");
    expect(commands.closeSession).toHaveBeenCalledTimes(1);
  });

  it("does not replay an uncertain destination launch", async () => {
    launched(begin());
    await jobs.finishHandoffWithSummary("Reviewed context");
    const requestId = jobs.handoffJob()!.launch!.requestId;
    jobs.applyHandoffLaunch({
      request_id: requestId,
      session: null,
      error: "Disconnected",
      uncertain: true,
    });
    await jobs.finishHandoffWithSummary("Reviewed context");
    expect(jobs.handoffJob()?.uncertain).toBe(true);
    expect(commands.launchBackgroundAgent).toHaveBeenCalledTimes(2);
    expect(commands.closeSession).not.toHaveBeenCalled();
  });

  it("recovers a selection failure without launching a second destination", async () => {
    launched(begin());
    await jobs.finishHandoffWithSummary("Reviewed context");
    commands.selectSession.mockRejectedValueOnce(new Error("queue full"));
    launched(jobs.handoffJob()!.launch!.requestId, "destination");
    await vi.waitFor(() => expect(jobs.handoffJob()?.error).toBe("queue full"));
    expect(commands.closeSession).not.toHaveBeenCalled();
    await jobs.completeHandoff();
    expect(jobs.handoffJob()).toBeNull();
    expect(commands.launchBackgroundAgent).toHaveBeenCalledTimes(2);
  });
});

describe("handoff polling", () => {
  it("does not reopen a dismissed dialog on each capture of the same brief", () => {
    launched(begin());
    const text = `${HANDOFF_MARK_START}\nbrief\n${HANDOFF_MARK_END}`;
    capture(text);
    expect(jobs.handoffJob()?.progressOpen).toBe(true);
    jobs.setHandoffProgressOpen(false);
    jobs.refreshHandoffProgress();
    capture(text);
    expect(jobs.handoffJob()?.progressOpen).toBe(false);
  });
  it("coalesces reads until the correlated reply and pauses on error", () => {
    launched(begin());
    for (let i = 0; i < 100; i += 1) jobs.refreshHandoffProgress();
    expect(commands.loadHandoffProgress).toHaveBeenCalledTimes(1);
    jobs.applyHandoffProgress({
      request_id: "unrelated-read",
      transcript: null,
      error: "unrelated",
    });
    jobs.refreshHandoffProgress();
    expect(commands.loadHandoffProgress).toHaveBeenCalledTimes(1);
    const requestId = jobs.handoffJob()!.readRequest!;
    jobs.applyHandoffProgress({ request_id: requestId, transcript: null, error: "Terminal ended" });
    jobs.refreshHandoffProgress();
    expect(commands.loadHandoffProgress).toHaveBeenCalledTimes(1);
    expect(jobs.handoffJob()?.readError).toBe("Terminal ended");
    jobs.retryHandoffProgress();
    expect(commands.loadHandoffProgress).toHaveBeenCalledTimes(2);
    expect(jobs.handoffJob()?.readRequest).not.toBe(requestId);
  });

  it("ignores a late capture from a cancelled job", async () => {
    launched(begin());
    const read = jobs.handoffJob()!.readRequest!;
    jobs.cancelHandoffJob();
    await vi.waitFor(() => expect(jobs.handoffJob()).toBeNull());
    launched(begin(), "new-summarizer");
    jobs.applyHandoffProgress({ request_id: read, transcript: null, error: "old capture failed" });
    expect(jobs.handoffJob()?.readError).toBeNull();
    expect(jobs.handoffJob()?.summarizerSession).toBe("new-summarizer");
  });
});
