import { invoke } from "@tauri-apps/api/core";
import { setHarnessStore } from "../store/harnessStore";
import type { HarnessAdvanceAction, HarnessArtifactKind, HarnessStep } from "./types";

/**
 * Harness reads and writes.
 *
 * They go to the workbench worker, not to the thread that carries keystrokes:
 * `features.json` and the markdown under `harness/` are files the daemon
 * parses on the spot, so a spec parse queued in front of typing would freeze
 * it (AGENTS.md).
 *
 * The exceptions are below, on the runtime channel: cancelling a job, binding
 * a session and driving the preview are all things about a *terminal*, and
 * they ack immediately.
 */
async function send(command: Record<string, unknown>): Promise<void> {
  await invoke("send_workbench_command", { command });
}

async function runtime(command: Record<string, unknown>): Promise<void> {
  await invoke("send_runtime_command", { command });
}

export async function loadFeatures(project: string): Promise<void> {
  setHarnessStore({ loadingList: true, listError: null });
  try {
    await send({ type: "load_harness_features", project });
  } catch (error) {
    setHarnessStore({ loadingList: false, listError: messageOf(error) });
  }
}

/** The feature and its timeline together: the tab shows both. */
export async function loadFeatureDetail(project: string, feature: number): Promise<void> {
  setHarnessStore({ loadingDetail: true, detailError: null });
  try {
    await send({ type: "load_harness_detail", project, feature });
  } catch (error) {
    setHarnessStore({ loadingDetail: false, detailError: messageOf(error) });
  }
}

export async function loadArtifact(
  project: string,
  feature: number,
  artifact: HarnessArtifactKind,
): Promise<void> {
  setHarnessStore({ loadingArtifact: true, artifactError: null, artifact: null });
  try {
    await send({ type: "load_harness_artifact", project, feature, artifact });
  } catch (error) {
    setHarnessStore({
      loadingArtifact: false,
      artifactError: messageOf(error),
    });
  }
}

/**
 * Approve, revise, block, or start a step.
 *
 * Approving is the whole message: the daemon starts the implementation step
 * itself, so the run continues even if this window goes away.
 */
export async function advance(
  project: string,
  feature: number,
  action: HarnessAdvanceAction,
  revision: number | null = null,
): Promise<void> {
  setHarnessStore("advancing", true);
  try {
    await send({ type: "harness_advance", project, feature, revision, action });
  } catch {
    setHarnessStore("advancing", false);
  }
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export async function runStep(project: string, feature: number, step: HarnessStep): Promise<void> {
  await send({ type: "run_harness_step", project, feature, step });
}

export async function registerFeature(
  project: string,
  specRaw: string,
  title: string | null = null,
  workspace: string | null = null,
): Promise<void> {
  await send({
    type: "register_harness_feature",
    project,
    workspace,
    spec_raw: specRaw,
    title,
  });
}

export async function registerFromIssue(
  project: string,
  issue: number,
  workspace: string | null = null,
): Promise<void> {
  await send({ type: "register_harness_from_issue", project, workspace, issue });
}

/** Run the harness's own checks; `ok: false` is an answer, not a failure. */
export async function validateHarness(project: string): Promise<void> {
  await send({ type: "validate_harness", project });
}

export async function listJobs(): Promise<void> {
  await send({ type: "list_jobs" });
}

// --- Runtime channel --------------------------------------------------------

export async function cancelJob(jobId: string): Promise<void> {
  await runtime({ type: "cancel_job", job_id: jobId });
}

export async function readJobLog(jobId: string): Promise<void> {
  await runtime({ type: "read_job_log", job_id: jobId });
}

/** Bind a feature to the orchestrator session running it. */
export async function linkSession(
  project: string,
  feature: number,
  sessionId: string,
): Promise<void> {
  await runtime({ type: "link_harness_session", project, feature, session_id: sessionId });
}

/**
 * Point the preview terminal at a session (§5.8).
 *
 * One at a time: attaching replaces whatever it held, which is what makes
 * clicking through a feature's sessions cheap. Refused for the session the
 * main terminal pane is already on — that one is not a preview, it is where
 * the user is.
 */
export async function attachPreview(
  sessionId: string,
  size: { cols: number; rows: number; pixel_width: number; pixel_height: number } | null = null,
): Promise<void> {
  await runtime({ type: "attach_harness_preview", session_id: sessionId, size });
}

/**
 * Tell the preview the geometry its canvas can actually paint.
 *
 * Dropped while nothing is attached, and a no-op when the size has not moved:
 * the pane calls it from a `ResizeObserver`, which fires once per frame of a
 * drag, and each one that got through would cost the daemon a resize plus a
 * full resync of the grid.
 */
export async function resizePreview(
  cols: number,
  rows: number,
  pixelWidth: number,
  pixelHeight: number,
): Promise<void> {
  await runtime({
    type: "resize_harness_preview",
    size: { cols, rows, pixel_width: pixelWidth, pixel_height: pixelHeight },
  });
}

export async function detachPreview(): Promise<void> {
  await runtime({ type: "detach_harness_preview" });
}

/** A key press for the preview terminal; never bracketed, never echoed. */
export async function sendPreviewKey(key: {
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
}): Promise<void> {
  await runtime({ type: "input_preview", key });
}
