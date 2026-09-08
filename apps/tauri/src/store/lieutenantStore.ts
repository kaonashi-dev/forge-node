import { createStore } from "solid-js/store";
import type { Job } from "../runtime/types";
import { answerLines, jobIsFinal } from "../panels/lieutenant";
import { EMPTY_OUTPUT, foldOutput, type JobOutput } from "./jobOutput";

export type LieutenantTurn = {
  question: string;
  answer: string[];
  running: boolean;
};

const JOB_TAIL_LINES = 400;

export const [lieutenantStore, setLieutenantStore] = createStore({
  project: null as string | null,
  turns: [] as LieutenantTurn[],
  jobId: null as string | null,
  providerSession: null as string | null,
  error: null as string | null,
  output: {} as Record<string, JobOutput>,
});

export function focusLieutenantProject(project: string | null): void {
  if (lieutenantStore.project === project) return;
  setLieutenantStore({
    project,
    turns: [],
    jobId: null,
    providerSession: null,
    error: null,
    output: {},
  });
}

export function startLieutenantTurn(question: string): void {
  setLieutenantStore("error", null);
  setLieutenantStore("turns", (turns) => [...turns, { question, answer: [], running: true }]);
}

export function setLieutenantError(error: string, project?: string): void {
  if (project !== undefined && lieutenantStore.project !== project) return;
  setLieutenantStore({ error, jobId: null });
  setLieutenantStore("turns", (turns) => {
    const last = turns.at(-1);
    if (!last || !last.running) return turns;
    return [...turns.slice(0, -1), { ...last, running: false }];
  });
}

export function clearLieutenant(): void {
  // Forget the conversation, but keep an active job armed so Clear cannot
  // allow a second question while the first provider process is still running.
  setLieutenantStore({ turns: [], providerSession: null, error: null, output: {} });
}

export function acceptLieutenantJob(project: string, job: Job): void {
  if (lieutenantStore.project !== project) return;
  setLieutenantStore({ jobId: job.id, error: null });
  applyJobState(job);
}

export function applyLieutenantJob(job: Job): void {
  if (lieutenantStore.jobId !== job.id) return;
  applyJobState(job);
}

function applyJobState(job: Job): void {
  if (!jobIsFinal(job.state)) return;
  if (job.provider_session_id) {
    setLieutenantStore("providerSession", job.provider_session_id);
  }
  setLieutenantStore("jobId", null);
  setLieutenantStore("turns", (turns) => {
    const last = turns.at(-1);
    if (!last) return turns;
    return [...turns.slice(0, -1), { ...last, running: false }];
  });
}

export function applyLieutenantOutput(jobId: string, fromLine: number, lines: string[]): void {
  foldIntoStore(jobId, fromLine, lines, true);
}

export function seedLieutenantOutput(jobId: string, lines: string[]): void {
  // A seed fills gaps around what already arrived live; the live copy wins.
  const from = Math.max(0, lines.length - JOB_TAIL_LINES);
  foldIntoStore(jobId, from, lines.slice(from), false);
}

/**
 * Fold a batch in and re-derive the turn's answer from the tail.
 *
 * The append case writes each line at its own store path so the array the
 * panel is reading keeps its identity — see `store/jobOutput.ts` for why a
 * fresh copy per batch is not affordable on the delta path.
 */
function foldIntoStore(jobId: string, fromLine: number, lines: string[], overwrite: boolean): void {
  const current = lieutenantStore.output[jobId] ?? EMPTY_OUTPUT;
  const plan = foldOutput(current, fromLine, lines, JOB_TAIL_LINES, overwrite);
  if (plan.kind === "none") return;
  if (plan.kind === "replace") {
    setLieutenantStore("output", jobId, plan.next);
  } else {
    for (let index = 0; index < plan.lines.length; index += 1) {
      setLieutenantStore("output", jobId, "lines", plan.at + index, plan.lines[index]);
    }
  }
  refreshTurnAnswer(jobId, lieutenantStore.output[jobId]?.lines ?? []);
}

function refreshTurnAnswer(jobId: string, output: Array<string | undefined>): void {
  const last = lieutenantStore.turns.at(-1);
  if (lieutenantStore.jobId !== jobId && last?.running !== false) return;
  const answer = answerLines(output);
  setLieutenantStore("turns", (turns) => {
    const current = turns.at(-1);
    if (!current) return turns;
    return [...turns.slice(0, -1), { ...current, answer }];
  });
}
