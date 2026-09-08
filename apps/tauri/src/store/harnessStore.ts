import { createStore } from "solid-js/store";
import type { HarnessArtifactKind, HarnessEvent, HarnessFeature } from "../harness/types";
import type { Job } from "../runtime/types";
import { EMPTY_OUTPUT, foldOutput, seedOutput, type JobOutput } from "./jobOutput";

/**
 * The harness, as far as the GUI is concerned.
 *
 * Scoped to one project: `harness/features.json` is one file per repository,
 * so a second project's features are a different list, and keeping both would
 * mean deciding which one an un-scoped action meant.
 *
 * Nothing here is authoritative. Every write goes to the daemon and the answer
 * it sends back is what lands — the tab never guesses what an approve did.
 */
export const [harnessStore, setHarnessStore] = createStore({
  /** The project the lists below belong to, so a stale answer is dropped. */
  project: null as string | null,
  features: [] as HarnessFeature[],
  /** `false` when the project has no `harness/` directory at all. */
  initialized: true,
  listError: null as string | null,
  loadingList: false,

  /** The feature the centre tab is on, `null` when it is showing the list. */
  openFeature: null as number | null,
  detail: null as HarnessFeature | null,
  timeline: [] as HarnessEvent[],
  detailError: null as string | null,
  loadingDetail: false,
  /**
   * An advance is in flight.
   *
   * The gate buttons disable on it: approving twice starts the implement step
   * twice, and the daemon would run both.
   */
  advancing: false,

  /** The artefact document on screen, and which one it is. */
  artifact: null as { kind: HarnessArtifactKind; text: string } | null,
  artifactError: null as string | null,
  loadingArtifact: false,

  /** Jobs this feature started, newest last. */
  jobs: [] as Job[],
  /** Output of the job the tab is streaming, by job id. */
  output: {} as Record<string, JobOutput>,
  /** The job whose stream the tab is showing. */
  openJob: null as string | null,

  /** `harness validate` output, kept verbatim — it is the report. */
  validate: null as { ok: boolean; output: string } | null,

  /** The session the preview terminal is watching, `null` when detached. */
  previewSession: null as string | null,
  /** Whether the preview takes the tab rather than a corner of it. */
  previewFull: false,

  /**
   * The number the daemon minted for the last registration.
   *
   * The draft tab has no id of its own to wait on, and the register command
   * acks without one; this is how it learns which feature it just made so it
   * can hand the person the tab for it.
   */
  registered: null as number | null,
});

/** How many lines of one job's stream are kept; the rest are on disk. */
const JOB_TAIL_LINES = 2000;

/**
 * Point the harness at a project.
 *
 * Clears everything: a feature id means nothing outside the repository whose
 * `features.json` numbered it, so carrying one across would open the wrong
 * feature under a right-looking number.
 */
export function focusHarnessProject(project: string | null): void {
  if (harnessStore.project === project) return;
  setHarnessStore({
    project,
    features: [],
    initialized: true,
    listError: null,
    loadingList: false,
    openFeature: null,
    detail: null,
    timeline: [],
    detailError: null,
    loadingDetail: false,
    advancing: false,
    artifact: null,
    artifactError: null,
    loadingArtifact: false,
    jobs: [],
    output: {},
    openJob: null,
    validate: null,
    registered: null,
    previewFull: false,
  });
}

export function applyFeatureList(
  project: string,
  features: HarnessFeature[],
  initialized: boolean,
): void {
  if (harnessStore.project !== project) return;
  setHarnessStore({ features, initialized, listError: null, loadingList: false });
  const open = harnessStore.detail;
  if (!open) return;
  const fresh = features.find((item) => item.id === open.id);
  if (fresh && (fresh.revision ?? 0) >= (open.revision ?? 0)) {
    setHarnessStore("detail", fresh);
  }
}

export function applyFeatureDetail(
  project: string,
  detail: HarnessFeature,
  timeline: HarnessEvent[],
): void {
  if (harnessStore.project !== project) return;
  // A tab switch leaves an in-flight read behind; applying it would paint
  // feature N's gate onto feature M.
  if (harnessStore.openFeature !== null && harnessStore.openFeature !== detail.id) {
    setHarnessStore("loadingDetail", false);
    return;
  }
  const current = harnessStore.detail;
  // An advance already wrote a newer row into `detail`; a `load_harness_detail`
  // that left before that write must not put the gate back on screen.
  if (current?.id === detail.id && (current.revision ?? 0) > (detail.revision ?? 0)) {
    setHarnessStore({
      detailError: null,
      loadingDetail: false,
      advancing: false,
    });
    return;
  }
  setHarnessStore({
    detail,
    timeline,
    detailError: null,
    loadingDetail: false,
    advancing: false,
  });
  mergeFeature(detail);
}

/**
 * Fold one feature's new state into the list.
 *
 * An advance answers with the feature alone, and the panel behind the tab is
 * showing the same row: re-reading the whole list to move one badge would be a
 * second round trip for a value already in hand.
 */
export function mergeFeature(feature: HarnessFeature): void {
  // Indexed write, not a rebuilt array: a project with a long feature list
  // gets one event per advance, and replacing the array replaces every row's
  // identity along with it.
  const index = harnessStore.features.findIndex((item) => item.id === feature.id);
  if (index < 0) {
    setHarnessStore("features", harnessStore.features.length, feature);
  } else if ((harnessStore.features[index]?.revision ?? 0) <= (feature.revision ?? 0)) {
    setHarnessStore("features", index, feature);
  }
  // The tab draws `detail` when the ids match. Leaving it behind the list is
  // how Approve left the gate on screen while the panel already said building.
  if (
    harnessStore.detail?.id === feature.id &&
    (harnessStore.detail.revision ?? 0) <= (feature.revision ?? 0)
  ) {
    setHarnessStore("detail", feature);
  }
}

export function openFeature(id: number | null): void {
  if (harnessStore.openFeature === id) return;
  // The artefact and the job stream belong to the feature that was open, not
  // to the one being opened. Loading flags too: a read still in flight for
  // the previous id would otherwise leave `Reading…` up forever, because its
  // answer is dropped and the new tab sees `loadingArtifact` already true.
  setHarnessStore({
    openFeature: id,
    detail: null,
    timeline: [],
    detailError: null,
    loadingDetail: false,
    artifact: null,
    artifactError: null,
    loadingArtifact: false,
    openJob: null,
    validate: null,
  });
}

export function applyHarnessJob(job: Job): void {
  const index = harnessStore.jobs.findIndex((item) => item.id === job.id);
  setHarnessStore("jobs", index < 0 ? harnessStore.jobs.length : index, job);
}

/**
 * Append a run of streamed lines at `fromLine`.
 *
 * The daemon numbers them, so a batch that arrives out of order lands where it
 * belongs rather than at the end. Only the tail is kept; the full transcript
 * is a file, and `log_path` names it.
 *
 * The append is written line by line at its own store path rather than by
 * handing back a new array. A run at 1 000 lines/s is ~125 batches a second,
 * and a fresh 2 000-entry copy per batch is the kind of per-delta cost
 * `docs/performance.md` rules out; `foldOutput` decides when a copy is
 * genuinely owed.
 */
export function applyHarnessOutput(jobId: string, fromLine: number, lines: string[]): void {
  foldIntoStore(jobId, fromLine, lines, true);
}

export function seedHarnessOutput(jobId: string, lines: string[]): void {
  setHarnessStore("output", jobId, seedOutput(lines, JOB_TAIL_LINES));
}

function foldIntoStore(jobId: string, fromLine: number, lines: string[], overwrite: boolean): void {
  const current = harnessStore.output[jobId] ?? EMPTY_OUTPUT;
  const plan = foldOutput(current, fromLine, lines, JOB_TAIL_LINES, overwrite);
  if (plan.kind === "none") return;
  if (plan.kind === "replace") {
    setHarnessStore("output", jobId, plan.next);
    return;
  }
  for (let index = 0; index < plan.lines.length; index += 1) {
    setHarnessStore("output", jobId, "lines", plan.at + index, plan.lines[index]);
  }
}

/** Jobs belonging to the feature the tab is on, oldest first. */
export function featureJobs(featureId: number | null): Job[] {
  if (featureId === null) return [];
  return harnessStore.jobs.filter((job) => job.feature_id === featureId);
}

export function harnessIsRunning(featureId: number | null): boolean {
  return featureJobs(featureId).some((job) => job.state === "Queued" || job.state === "Running");
}
