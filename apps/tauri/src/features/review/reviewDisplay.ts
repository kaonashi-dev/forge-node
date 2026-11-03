import type { Finding, ReviewRun, ReviewStage, Severity, Verdict } from "../../contracts/workbench";
import type { BadgeTone } from "../../ui/Badge";

export type Grouping = "severity" | "file";

export type FindingGroup = { key: string; findings: Finding[] };

const SEVERITIES: Severity[] = ["P0", "P1", "P2"];
const STAGES: ReviewStage[] = ["Diff", "Pack", "Model", "Validate"];

const byLine = (a: Finding, b: Finding) => (a.line ?? 0) - (b.line ?? 0);

/** General findings (no line) lead their file; empty groups are left out. */
export function groupFindings(findings: readonly Finding[], by: Grouping): FindingGroup[] {
  if (by === "severity") {
    return SEVERITIES.map((key) => ({
      key,
      findings: findings
        .filter((finding) => finding.severity === key)
        .sort((a, b) => a.path.localeCompare(b.path) || byLine(a, b)),
    })).filter((group) => group.findings.length > 0);
  }
  const paths = [...new Set(findings.map((finding) => finding.path))].sort();
  return paths.map((key) => ({
    key,
    findings: findings.filter((finding) => finding.path === key).sort(byLine),
  }));
}

export function verdictLabel(verdict: Verdict): string {
  return { Block: "Bloquea", Review: "Revisar", Ok: "OK" }[verdict];
}

export function verdictTone(verdict: Verdict): BadgeTone {
  return ({ Block: "bad", Review: "warn", Ok: "good" } as const)[verdict];
}

export function severityTone(severity: Severity): BadgeTone {
  return ({ P0: "bad", P1: "warn", P2: "neutral" } as const)[severity];
}

/** An unmeasured head (`null`) is not evidence the checkout moved. */
export function isStale(run: ReviewRun, head: string | null | undefined): boolean {
  return head != null && head !== run.head_oid;
}

export function stageLabel(stage: ReviewStage | null): string {
  switch (stage) {
    case "Diff":
      return "Reading the diff";
    case "Pack":
      return "Building the pack";
    case "Model":
      return "Waiting for the reviewer";
    case "Validate":
      return "Validating findings";
    default:
      return "Queued";
  }
}

/** Percent, halfway into the current stage: none of them reports its own progress. */
export function stageProgress(stage: ReviewStage | null): number {
  if (stage === null) return 0;
  return ((STAGES.indexOf(stage) + 0.5) / STAGES.length) * 100;
}

export function duration(run: Pick<ReviewRun, "created_at" | "finished_at">, now: number): string {
  const end = run.finished_at ? Date.parse(run.finished_at) : now;
  const seconds = Math.max(0, Math.round((end - Date.parse(run.created_at)) / 1000));
  const pad = (n: number) => String(n).padStart(2, "0");
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${pad(seconds % 60)}s`;
  return `${Math.floor(seconds / 3600)}h ${pad(Math.floor((seconds % 3600) / 60))}m`;
}

export function scoreLabel(score: number | null): string {
  return score === null ? "—" : `${score.toFixed(1)} / 5`;
}
