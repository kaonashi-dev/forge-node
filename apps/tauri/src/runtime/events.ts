import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { toast } from "../ui";
import type { CellsPayload } from "../terminal/types";
import { applyShellSnapshot, forgeStore } from "../store/forgeStore";
import { adoptPendingCompose } from "../store/prComposeStore";
import { adoptPendingReviews } from "../store/prReviewStore";
import { loadFeatureDetail } from "../harness/api";
import { readJobLog } from "./api";
import {
  acceptLieutenantJob,
  applyLieutenantJob,
  applyLieutenantOutput,
  seedLieutenantOutput,
  setLieutenantError,
} from "../store/lieutenantStore";
import { applyTranscript, failTranscript, setRuntimeStore } from "../store/runtimeStore";
import { asSessionTranscript } from "./externalTranscript";
import { applySessionChanges, failSessionChanges } from "../store/sessionChangesStore";
import { setLoading, setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import { refreshDiff } from "../workbench/decorations";
import { previewImageReader } from "../workbench/api";
import { dataUrl } from "../workbench/previewImages";
import type {
  Branches,
  FileContents,
  FileTree,
  ImageContents,
  JuvaDraft,
  RebaseState,
  SearchResults,
  SessionChanges,
  SessionTranscript,
  UsageAnalytics,
  WorkspaceDiff,
  WorkspaceReview,
} from "../workbench/types";
import {
  applyFeatureDetail,
  applyFeatureList,
  applyHarnessJob,
  applyHarnessOutput,
  harnessStore,
  mergeFeature,
  seedHarnessOutput,
  setHarnessStore,
} from "../store/harnessStore";
import type { HarnessArtifactKind, HarnessEvent, HarnessFeature } from "../harness/types";
import { cellsChannel, clipboardChannel, previewCellsChannel } from "./bus";
import type {
  ConnectedPayload,
  DisconnectedPayload,
  ExternalTranscript,
  Job,
  ShareAction,
  ShareCandidate,
  ShareStatusEntry,
  ShareTrigger,
  StatePayload,
} from "./types";
import {
  applyCandidates,
  applyShareActions,
  applySharesApplied,
  applyShareStatus,
  setSharesError,
} from "../store/sharesStore";

export function applyConnected(payload: ConnectedPayload): void {
  applyShellSnapshot(payload.store);
  adoptPendingLaunches();
  setRuntimeStore({
    connection: {
      kind: "connected",
      instanceId: payload.daemon.instance_id,
      version: payload.daemon.daemon_version,
    },
    activeSession: payload.active_session,
    activeTerminal: payload.active_terminal,
  });
}

/** Agent launches that outlive their tab: bind them once the snapshot lands. */
function adoptPendingLaunches(): void {
  adoptPendingReviews(forgeStore.sessions);
  adoptPendingCompose(forgeStore.sessions);
}

/** `[workspace, payload]` — the worker tags every answer with what it is about. */
type Tagged<T> = [string, T];

type Failure = { workspace: string | null; error: string };

type SessionFailure = { session: string; error: string };

/**
 * Workbench answers.
 *
 * Every one is checked against the workspace the panels are looking at: a read
 * started before a session switch lands after it, and painting the previous
 * checkout's diff under the new branch name is worse than painting nothing.
 */
async function bindWorkbenchEvents(): Promise<UnlistenFn[]> {
  const forCurrent = (workspace: string | null): boolean =>
    workspace !== null && workspace === workbenchStore.workspace;

  const answer = <T>(
    event: string,
    surface: string,
    apply: (payload: T) => void,
  ): Promise<UnlistenFn> =>
    listen<Tagged<T>>(event, ({ payload: [workspace, value] }) => {
      if (!forCurrent(workspace)) return;
      apply(value);
      setLoading(surface, false);
    });

  const failure = (event: string, surface: string, field: string): Promise<UnlistenFn> =>
    listen<Failure>(event, ({ payload }) => {
      if (payload.workspace === null || forCurrent(payload.workspace)) {
        setWorkbenchStore(field as "diffError", payload.error);
        setLoading(surface, false);
      }
    });

  /* Keyed on the session, not the checkout: the split lives beside one
     terminal, and a session's answer is still its own after the workbench has
     been pointed somewhere else. */
  const sessionAnswer = <T>(event: string, apply: (session: string, value: T) => void) =>
    listen<Tagged<T>>(event, ({ payload: [session, value] }) => apply(session, value));

  return Promise.all([
    answer<WorkspaceDiff>("workbench:diff", "diff", (diff) =>
      setWorkbenchStore({ diff, diffError: null }),
    ),
    failure("workbench:diff_failed", "diff", "diffError"),
    answer<WorkspaceReview>("workbench:review", "review", (review) =>
      setWorkbenchStore({ review, reviewError: null, reviewAt: Date.now() }),
    ),
    failure("workbench:review_failed", "review", "reviewError"),
    sessionAnswer<SessionChanges>("workbench:session_changes", applySessionChanges),
    listen<SessionFailure>("workbench:session_changes_failed", ({ payload }) =>
      failSessionChanges(payload.session, payload.error),
    ),
    sessionAnswer<SessionTranscript>("workbench:session_transcript", applyTranscript),
    listen<SessionFailure>("workbench:session_transcript_failed", ({ payload }) =>
      failTranscript(payload.session, payload.error),
    ),
    /* A discovered run resolves into the same handoff slot as a live one: both
       are keyed by the correlation id the request carried, and only the shape
       of the capture differs. */
    sessionAnswer<ExternalTranscript>("workbench:external_transcript", (session, transcript) =>
      applyTranscript(session, asSessionTranscript(transcript)),
    ),
    listen<SessionFailure>("workbench:external_transcript_failed", ({ payload }) =>
      failTranscript(payload.session, payload.error),
    ),
    answer<FileTree>("workbench:file_tree", "tree", (tree) =>
      setWorkbenchStore({ tree, treeError: null }),
    ),
    failure("workbench:file_tree_failed", "tree", "treeError"),
    answer<FileContents>("workbench:file", "file", (file) =>
      setWorkbenchStore({ file, fileError: null }),
    ),
    answer<FileContents>("workbench:file_saved", "file", (file) => {
      setWorkbenchStore({ file, fileError: null });
      // The write may have changed marks the gutter already shows.
      refreshDiff();
    }),
    failure("workbench:file_failed", "file", "fileError"),
    failure("workbench:save_failed", "file", "fileError"),
    /* Not checked against the current workspace: the reader matches on the
       workspace it was asked for, and a preview that moved on has stopped
       listening for the answer. */
    listen<Tagged<ImageContents>>("workbench:image", ({ payload: [workspace, image] }) =>
      previewImageReader.settle(workspace, image.path, { url: dataUrl(image) }),
    ),
    listen<{ workspace: string; path: string; error: string }>(
      "workbench:image_failed",
      ({ payload }) =>
        previewImageReader.settle(payload.workspace, payload.path, { error: payload.error }),
    ),
    answer<SearchResults>("workbench:search", "search", (search) => setWorkbenchStore({ search })),
    failure("workbench:search_failed", "search", "fileError"),
    answer<RebaseState>("workbench:rebase", "rebase", (rebase) =>
      setWorkbenchStore({ rebase, rebaseError: null }),
    ),
    failure("workbench:rebase_failed", "rebase", "rebaseError"),
    // Branches are a project's, not a workspace's, so they carry no tag to
    // check and land whoever asked.
    listen<{
      project: string;
      branches: Branches["branches"];
      remotes: Branches["remotes"];
      default_branch: string | null;
    }>("workbench:branches", ({ payload }) => {
      setLoading("branches", false);
      setWorkbenchStore("branches", {
        branches: payload.branches,
        remotes: payload.remotes,
        default_branch: payload.default_branch,
      });
    }),
    listen<UsageAnalytics>("workbench:usage", ({ payload }) => {
      setLoading("usage", false);
      setWorkbenchStore({ usage: payload, usageError: null });
    }),
    listen<{ error: string }>("workbench:usage_failed", ({ payload }) => {
      setLoading("usage", false);
      setWorkbenchStore("usageError", payload.error);
    }),
    // Juva acks when the draft starts, so the text arrives on the runtime
    // thread's own event, not as a workbench answer.
    listen<{ workspace: string; draft: JuvaDraft; fell_back: boolean }>(
      "runtime:juva_draft",
      ({ payload }) => {
        setLoading("juva", false);
        if (!forCurrent(payload.workspace)) return;
        setWorkbenchStore({ juvaDraft: payload.draft, juvaError: null });
      },
    ),
    listen<{ workspace: string | null; error: string }>("workbench:juva_failed", ({ payload }) => {
      setLoading("juva", false);
      setWorkbenchStore("juvaError", payload.error);
    }),
    listen<string>("workbench:juva_applied", () => {
      setWorkbenchStore({ juvaDraft: null, juvaError: null });
    }),
    // Shared files (§14.2). Candidates and rules belong to a *project*, so
    // they carry no workspace tag to check; status and plans do.
    listen<{ project: string; candidates: ShareCandidate[]; truncated: boolean }>(
      "workbench:share_candidates",
      ({ payload }) => applyCandidates(payload.project, payload.candidates, payload.truncated),
    ),
    listen<Failure>("workbench:share_candidates_failed", ({ payload }) =>
      setSharesError(payload.error),
    ),
    listen<{ workspace: string | null; actions: ShareAction[] }>(
      "workbench:share_plan",
      ({ payload }) => {
        if (payload.workspace) applyShareActions(payload.workspace, payload.actions);
      },
    ),
    listen<Failure>("workbench:share_plan_failed", ({ payload }) => setSharesError(payload.error)),
    listen<{ workspace: string; entries: ShareStatusEntry[] }>(
      "workbench:share_status",
      ({ payload }) => applyShareStatus(payload.workspace, payload.entries),
    ),
    listen<Failure>("workbench:share_status_failed", ({ payload }) =>
      setSharesError(payload.error),
    ),
    listen<Failure>("workbench:share_store_failed", ({ payload }) => setSharesError(payload.error)),
  ]);
}

/** `{ project, feature, error }` — a harness answer that failed. */
type HarnessFailure = { project: string; feature: number | null; error: string };

/**
 * Harness answers.
 *
 * Every one carries the project it is about and is dropped unless the panels
 * are still on it: `harness/features.json` is one file per repository, so a
 * feature id from another project would open a right-looking wrong row.
 */
async function bindHarnessEvents(): Promise<UnlistenFn[]> {
  const forCurrent = (project: string): boolean => project === harnessStore.project;

  return Promise.all([
    listen<[string, { project: string | null; features: HarnessFeature[]; initialized: boolean }]>(
      "harness:features",
      ({ payload: [project, list] }) => {
        applyFeatureList(project, list.features, list.initialized);
      },
    ),
    listen<[string, HarnessFeature, HarnessEvent[]]>(
      "harness:detail",
      ({ payload: [project, detail, timeline] }) => {
        applyFeatureDetail(project, detail, timeline);
      },
    ),
    // An advance answers with the feature's new state, so the tab and the list
    // both redraw from the answer rather than from a guess about what it did.
    listen<[string, HarnessFeature]>("harness:advanced", ({ payload: [project, feature] }) => {
      if (!forCurrent(project)) return;
      setHarnessStore("advancing", false);
      mergeFeature(feature);
      if (harnessStore.openFeature !== feature.id) return;
      // mergeFeature only writes `detail` when it already holds this id; an
      // Approve that lands before the first detail read still has to put the
      // new row on the tab so the gate leaves the screen.
      setHarnessStore("detail", feature);
      void loadFeatureDetail(project, feature.id).catch(() => undefined);
    }),
    listen<[string, HarnessFeature]>("harness:registered", ({ payload: [project, feature] }) => {
      if (!forCurrent(project)) return;
      mergeFeature(feature);
      setHarnessStore("registered", feature.id);
    }),
    listen<[string, number, HarnessArtifactKind, string]>(
      "harness:artifact",
      ({ payload: [project, feature, kind, text] }) => {
        if (!forCurrent(project) || harnessStore.openFeature !== feature) return;
        setHarnessStore({
          artifact: { kind, text },
          artifactError: null,
          loadingArtifact: false,
        });
      },
    ),
    // A missing artefact is the normal state of a feature that has not reached
    // that step yet: the reason goes where the document would have been.
    listen<HarnessFailure>("harness:artifact_failed", ({ payload }) => {
      if (!forCurrent(payload.project)) return;
      if (payload.feature !== null && harnessStore.openFeature !== payload.feature) return;
      setHarnessStore({
        artifactError: payload.error,
        artifact: null,
        loadingArtifact: false,
      });
    }),
    listen<[string, number, Job]>("harness:step_started", ({ payload: [project, , job] }) => {
      if (!forCurrent(project)) return;
      applyHarnessJob(job);
      setHarnessStore("openJob", job.id);
      void readJobLog(job.id).catch(() => undefined);
    }),
    // `ok: false` is an answer, not a failure — the output is the report the
    // user asked for, and it is the interesting case.
    listen<[string, boolean, string]>("harness:validated", ({ payload: [project, ok, output] }) => {
      if (!forCurrent(project)) return;
      setHarnessStore("validate", { ok, output });
    }),
    listen<Job[]>("harness:jobs", ({ payload }) => {
      for (const job of payload) applyHarnessJob(job);
    }),
    listen<HarnessFailure>("harness:failed", ({ payload }) => {
      if (!forCurrent(payload.project)) return;
      setHarnessStore({
        advancing: false,
        loadingList: false,
        loadingDetail: false,
      });
      if (payload.feature === null) setHarnessStore("listError", payload.error);
      else setHarnessStore("detailError", payload.error);
    }),
  ]);
}

/**
 * Say that a headless run finished (§4.2 U16).
 *
 * Only the terminal states, and only once: `job_updated` fires for every
 * transition, and a toast per transition would be four for one run. A failure
 * is the one that has to be seen, so it is the one that does not time out.
 */
const announced = new Set<string>();

function announceJob(job: Job): void {
  if (job.state !== "Succeeded" && job.state !== "Failed") return;
  if (announced.has(job.id)) return;
  announced.add(job.id);
  if (job.state === "Succeeded") {
    toast({ title: `${job.role} finished`, tone: "success" });
    return;
  }
  toast({
    title: `${job.role} failed`,
    detail: job.exit_code === null ? undefined : `exit ${job.exit_code}`,
    tone: "danger",
  });
}

export async function bindRuntimeEvents(): Promise<UnlistenFn> {
  const unlisteners = await Promise.all([
    listen("runtime:connecting", () => {
      setRuntimeStore("connection", { kind: "connecting" });
    }),
    listen<ConnectedPayload>("runtime:connected", (event) => {
      applyConnected(event.payload);
    }),
    listen<StatePayload>("runtime:state", (event) => {
      applyShellSnapshot(event.payload.store);
      adoptPendingLaunches();
      setRuntimeStore("activeSession", event.payload.active_session);
      setRuntimeStore("activeTerminal", event.payload.active_terminal);
    }),
    // Terminal frames go straight to the canvas: see `runtime/bus.ts` for why
    // they must not pass through a Solid store.
    listen<CellsPayload>("runtime:cells", (event) => {
      cellsChannel.publish(event.payload);
    }),
    listen<{ text: string }>("runtime:clipboard", (event) => {
      clipboardChannel.publish(event.payload.text);
    }),
    ...(await bindWorkbenchEvents()),
    ...(await bindHarnessEvents()),
    // The preview is a second terminal on its own event: the main canvas must
    // not repaint because a watched harness session printed a line.
    listen<CellsPayload>("runtime:preview_cells", (event) => {
      previewCellsChannel.publish(event.payload);
    }),
    listen("runtime:preview_detached", () => {
      setHarnessStore("previewSession", null);
    }),
    listen<{ reason: string }>("runtime:notice", (event) => {
      setRuntimeStore("notice", event.payload.reason);
    }),
    // Provisioning acks when it *starts*; this is where a worktree stops
    // saying "setting up" and says what it got (§14.2).
    listen<{
      workspace: string;
      trigger: ShareTrigger;
      actions: ShareAction[];
      error: string | null;
    }>("runtime:shares_applied", (event) => {
      applySharesApplied(
        event.payload.workspace,
        event.payload.trigger,
        event.payload.actions,
        event.payload.error,
      );
      const fell = event.payload.actions.filter((action) => action.fallback).length;
      if (fell > 0) {
        toast({
          title: fell === 1 ? "One file was copied, not cloned" : `${fell} files were copied`,
          detail: "This volume has no copy-on-write, so the clone fell back to a full copy.",
        });
      }
    }),
    listen<{ workspace: string; reason: string }>("runtime:worktree_blocked", (event) => {
      setRuntimeStore("worktreeRemoval", {
        workspace: event.payload.workspace,
        reason: event.payload.reason,
      });
    }),
    listen<{ project: string; branch: string; reason: string }>(
      "runtime:worktree_create_failed",
      (event) => setRuntimeStore("worktreeCreationFailure", event.payload),
    ),
    listen<{ profile: string; error: string | null }>("runtime:profile_save", (event) => {
      setRuntimeStore("profileSave", event.payload);
    }),
    listen<{ project: string; job: Job }>("runtime:lieutenant_job", (event) => {
      acceptLieutenantJob(event.payload.project, event.payload.job);
      void readJobLog(event.payload.job.id).catch(() => undefined);
    }),
    listen<{ project: string; error: string }>("runtime:lieutenant_failed", (event) => {
      setLieutenantError(event.payload.error, event.payload.project);
    }),
    // One job stream, two readers: a job is either the lieutenant's answer or
    // a harness step, and each store ignores the ids that are not its own.
    listen<Job>("runtime:job_updated", (event) => {
      applyLieutenantJob(event.payload);
      applyHarnessJob(event.payload);
      announceJob(event.payload);
    }),
    listen<{ job_id: string; from_line: number; lines: string[] }>(
      "runtime:job_output",
      (event) => {
        applyLieutenantOutput(event.payload.job_id, event.payload.from_line, event.payload.lines);
        applyHarnessOutput(event.payload.job_id, event.payload.from_line, event.payload.lines);
      },
    ),
    listen<{ job_id: string; lines: string[] }>("runtime:job_log", (event) => {
      seedLieutenantOutput(event.payload.job_id, event.payload.lines);
      seedHarnessOutput(event.payload.job_id, event.payload.lines);
    }),
    listen<DisconnectedPayload>("runtime:disconnected", (event) => {
      setRuntimeStore("connection", {
        kind: "disconnected",
        reason: event.payload.reason,
      });
    }),
  ]);

  return () => {
    for (const unlisten of unlisteners) {
      unlisten();
    }
  };
}
