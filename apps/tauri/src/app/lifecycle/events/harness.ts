import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { toast } from "../../../ui/index";
import { loadFeatureDetail } from "../../../features/harness/api";
import { readJobLog } from "../../../features/harness/commands";
import { previewCellsChannel } from "../../../runtime/bus";
import {
  applyFeatureDetail,
  applyFeatureList,
  applyHarnessJob,
  applyHarnessOutput,
  harnessStore,
  mergeFeature,
  seedHarnessOutput,
  setHarnessStore,
} from "../../../features/harness/harnessStore";
import type { HarnessArtifactKind, HarnessEvent, HarnessFeature } from "../../../contracts/harness";
import type { CellsPayload } from "../../../contracts/terminal";
import type { Job } from "../../../contracts/runtime";

/** `{ project, feature, error }` — a harness answer that failed. */
type HarnessFailure = { project: string; feature: number | null; error: string };

/**
 * Harness answers.
 *
 * Every one carries the project it is about and is dropped unless the panels
 * are still on it: `harness/features.json` is one file per repository, so a
 * feature id from another project would open a right-looking wrong row.
 */
export function bindHarnessEvents(): Promise<UnlistenFn[]> {
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
    // The preview is a second terminal on its own event: the main canvas must
    // not repaint because a watched harness session printed a line.
    listen<CellsPayload>("runtime:preview_cells", (event) => {
      previewCellsChannel.publish(event.payload);
    }),
    listen("runtime:preview_detached", () => {
      setHarnessStore("previewSession", null);
    }),
    listen<Job>("runtime:job_updated", (event) => {
      applyHarnessJob(event.payload);
      announceJob(event.payload);
    }),
    listen<{ job_id: string; from_line: number; lines: string[] }>(
      "runtime:job_output",
      (event) => {
        applyHarnessOutput(event.payload.job_id, event.payload.from_line, event.payload.lines);
      },
    ),
    listen<{ job_id: string; lines: string[] }>("runtime:job_log", (event) => {
      seedHarnessOutput(event.payload.job_id, event.payload.lines);
    }),
  ]);
}

/**
 * Report a headless run's outcome.
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
