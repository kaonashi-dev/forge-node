#!/usr/bin/env bun
/**
 * Human interface of the subagent harness.
 *
 * State on disk (`features.json`, `progress/`, `specs/`), not in chat.
 * Callable as `scripts/harness <cmd>` or `bun harness/src/cli.ts <cmd>`.
 */

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  ACTIVE,
  CHECKOUT,
  FEATURES,
  HARNESS,
  PROGRESS,
  ROOT,
  SPECS,
  currentPath,
  isDir,
  isFile,
  load,
  padEnd,
  padStart,
  progressFile,
  rel,
  type Feature,
  type HarnessData,
} from "./harness.ts";
import {
  appendEvent,
  contextPath,
  eventsPath,
  gatePath,
  gateTemplate,
  readEvents,
  type EventType,
} from "./events.ts";
import { validate } from "./validate.ts";

/** What is missing to move forward, per status. */
const NEXT_HINT: Record<string, string> = {
  pending: "spec missing → in the agent: /feature (or resume if already registered)",
  spec_ready: "human gate → /feature-go {id}",
  in_progress: "the implementer has the ball; wait or check progress/current.md",
  in_review: "the reviewer has the ball (round {rounds})",
  done: "closed",
  blocked: "blocked → read progress/current.md and the last review_*.md",
};

/** Python's `str.format(id=…, rounds=…)`, restricted to those two keys. */
function fmtHint(tpl: string, vars: { id: number; rounds: number }): string {
  return tpl.replace(/\{(id|rounds)\}/g, (_, k: "id" | "rounds") => String(vars[k]));
}

function featureLine(f: Feature): string {
  const fid = f.id;
  const status = f.status;
  const rounds = f.review_rounds ?? 0;
  const hint = fmtHint(NEXT_HINT[status] ?? status, { id: fid, rounds });

  let extra = "";
  if (["spec_ready", "in_progress", "in_review", "done"].includes(status)) {
    if (!isDir(join(SPECS, `${fid}-${f.slug}`))) extra = " ⚠ spec missing";
  }
  if (status === "spec_ready" && !isFile(gatePath(fid))) extra += " ⚠ gate missing";
  if (["in_progress", "in_review"].includes(status) && !isFile(contextPath(fid))) {
    extra += " ⚠ context missing";
  }
  if (status === "in_review" || status === "done") {
    if (!isFile(progressFile("review", fid))) extra += " ⚠ review missing";
  }
  return `${padStart(String(fid), 3)}  ${padEnd(f.slug, 28)}  ${padEnd(status, 12)}  ${hint}${extra}`;
}

async function saveFeatures(data: HarnessData): Promise<void> {
  writeFileSync(FEATURES, `${JSON.stringify(data, null, 2)}\n`);
}

async function cmdList(): Promise<number> {
  const data = await load();
  const feats = data.features ?? [];
  console.log(`${padStart("id", 3)}  ${padEnd("slug", 28)}  ${padEnd("status", 12)}  next`);
  console.log(`${"─".repeat(3)}  ${"─".repeat(28)}  ${"─".repeat(12)}  ${"─".repeat(40)}`);
  for (const f of feats) console.log(featureLine(f));
  if (feats.length === 0) console.log('(no features — register one with /feature "…")');
  return 0;
}

async function cmdStatus(): Promise<number> {
  const data = await load();
  const feats = data.features ?? [];
  const active = feats.filter((f) => ACTIVE.has(f.status));
  const pending = feats.filter((f) => f.status === "pending");
  const ready = feats.filter((f) => f.status === "spec_ready");
  const blocked = feats.filter((f) => f.status === "blocked");

  console.log(`project: ${data.project ?? "?"}`);
  console.log(
    `features: ${feats.length}  ` +
      `(pending=${pending.length}, spec_ready=${ready.length}, ` +
      `active=${active.length}, blocked=${blocked.length})`,
  );
  console.log();

  const note = active[0] ?? ready[0] ?? pending[0];
  if (note !== undefined && isFile(currentPath(note.id))) {
    const text = (await Bun.file(currentPath(note.id)).text()).trim();
    for (const line of text.split("\n")) {
      if (line.startsWith("**Feature") || line.startsWith("**Status")) console.log(line);
    }
    const lower = text.toLowerCase();
    if (lower.includes("none") && lower.includes("idle")) {
      console.log(`${rel(currentPath(note.id))}: idle`);
    }
    console.log();
  }

  for (const f of feats) console.log(featureLine(f));

  console.log();
  const dirty = Bun.spawnSync(["git", "status", "--porcelain"], {
    cwd: CHECKOUT,
    stdout: "pipe",
    stderr: "pipe",
  });
  const out = new TextDecoder().decode(dirty.stdout).trim();
  if (out) {
    console.log(`working tree: ${out.split("\n").length} uncommitted change(s)`);
  } else {
    console.log("working tree: clean");
  }

  console.log();
  console.log(nextAction(feats));
  return 0;
}

function nextAction(feats: Feature[]): string {
  const active = feats.filter((f) => ACTIVE.has(f.status));
  if (active.length > 0) {
    const f = active[0]!;
    const where = f.workspace_path === undefined ? "" : ` in ${f.workspace_path}`;
    return (
      `next: feature ${f.id} (${f.slug}) is ${f.status}${where} — ` +
      `do not open another one in that checkout`
    );
  }
  const blocked = feats.filter((f) => f.status === "blocked");
  if (blocked.length > 0) {
    const f = blocked[0]!;
    return `next: unblock feature ${f.id} (${f.slug}) by reading progress/`;
  }
  const ready = feats.filter((f) => f.status === "spec_ready");
  if (ready.length > 0) {
    const f = ready[0]!;
    return `next: /feature-go ${f.id}  (spec ready: harness/specs/${f.id}-${f.slug}/)`;
  }
  const pending = feats.filter((f) => f.status === "pending");
  if (pending.length > 0) {
    const f = pending[0]!;
    return (
      `next: complete the spec of feature ${f.id} (${f.slug}) — ` +
      `in the agent: ask the lead to launch spec-author, or /feature with the same text`
    );
  }
  return 'next: /feature "<specification>" to register the first one';
}

function resumeHint(f: Feature): string {
  const id = f.id;
  switch (f.status) {
    case "pending":
      return `resume: launch spec-author for feature ${id} (${f.slug})`;
    case "spec_ready":
      return `resume: read ${rel(gatePath(id))} then /feature-go ${id}`;
    case "in_progress":
      return `resume: implementer in flight — check ${rel(currentPath(id))} and ${rel(eventsPath(id))}`;
    case "in_review":
      return `resume: reviewer in flight (round ${f.review_rounds ?? 0}) — check ${rel(progressFile("review", id))}`;
    case "blocked":
      return `resume: read ${rel(currentPath(id))} and last events in ${rel(eventsPath(id))}`;
    case "done":
      return `resume: feature ${id} is closed — review diff and commit if desired`;
    default:
      return `resume: unknown status '${f.status}'`;
  }
}

async function cmdActive(): Promise<number> {
  // One line per active feature: a project checked out into several worktrees
  // can have one running in each, so this is a list, not a single answer.
  for (const f of ((await load()).features ?? []).filter((x) => ACTIVE.has(x.status))) {
    console.log(`${f.id}|${f.slug}|${f.status}|${f.workspace_path ?? ""}`);
  }
  return 0;
}

/**
 * The checkout that owns the harness state — absolute, one line, nothing else.
 *
 * The one supported way to ask the question from outside this module. A
 * worktree's own `harness/` is a frozen copy from its commit, so an agent
 * standing in one has to prefix every state path with this answer instead of
 * writing beside itself. `harnessRoot()` resolves it once — `FORGE_HARNESS_ROOT`
 * first, then `git rev-parse --git-common-dir` — and this prints that, so
 * nothing has to re-derive the rule.
 */
function cmdRoot(): number {
  console.log(ROOT);
  return 0;
}

async function cmdNext(): Promise<number> {
  console.log(nextAction((await load()).features ?? []));
  return 0;
}

async function cmdResume(rawId: string | undefined): Promise<number> {
  const id = Number(rawId);
  if (rawId === undefined || !Number.isInteger(id)) {
    console.error(`invalid argument for 'resume': expected an integer id, not '${rawId ?? ""}'`);
    return 2;
  }
  const f = ((await load()).features ?? []).find((x) => x.id === id);
  if (f === undefined) {
    console.error(`there is no feature with id=${id}`);
    return 1;
  }
  console.log(resumeHint(f));
  const events = readEvents(id);
  if (events.length > 0) {
    const last = events[events.length - 1]!;
    console.log(`last event: ${last.type} @ ${last.ts}`);
  }
  console.log(nextAction([f]));
  return 0;
}

async function cmdTimeline(rawId: string | undefined): Promise<number> {
  const id = Number(rawId);
  if (rawId === undefined || !Number.isInteger(id)) {
    console.error(`invalid argument for 'timeline': expected an integer id, not '${rawId ?? ""}'`);
    return 2;
  }
  const f = ((await load()).features ?? []).find((x) => x.id === id);
  if (f === undefined) {
    console.error(`there is no feature with id=${id}`);
    return 1;
  }
  const events = readEvents(id);
  if (events.length === 0) {
    console.log(`(no events for feature ${id})`);
    return 0;
  }
  for (const e of events) {
    const extra = Object.entries(e)
      .filter(([k]) => k !== "ts" && k !== "type")
      .map(([k, v]) => `${k}=${JSON.stringify(v)}`)
      .join(" ");
    console.log(`${e.ts}  ${e.type}${extra ? `  ${extra}` : ""}`);
  }
  return 0;
}

async function cmdEvent(argv: string[]): Promise<number> {
  const [rawId, type, ...rest] = argv;
  const id = Number(rawId);
  if (!Number.isInteger(id) || !type) {
    console.error("usage: scripts/harness event <id> <type> [--data '{...}']");
    return 2;
  }
  let payload: Record<string, unknown> = {};
  const dataIdx = rest.indexOf("--data");
  if (dataIdx >= 0) {
    const raw = rest[dataIdx + 1];
    if (!raw) {
      console.error("event: --data requires a JSON object");
      return 2;
    }
    try {
      payload = JSON.parse(raw) as Record<string, unknown>;
    } catch (exc) {
      console.error(`event: invalid JSON: ${(exc as Error).message}`);
      return 2;
    }
  }
  appendEvent(id, { type: type as EventType, ...payload });
  console.log(`appended ${type} -> ${rel(eventsPath(id))}`);
  return 0;
}

async function cmdFromIssue(raw: string | undefined): Promise<number> {
  if (!raw) {
    console.error("usage: scripts/harness from-issue <number|url>");
    return 2;
  }
  const match = raw.match(/(\d+)\s*$/);
  const issueNum = match ? Number(match[1]) : Number(raw);
  if (!Number.isInteger(issueNum) || issueNum < 1) {
    console.error(`from-issue: could not parse issue number from '${raw}'`);
    return 2;
  }

  const gh = Bun.spawnSync(
    ["gh", "issue", "view", String(issueNum), "--json", "title,body,number"],
    { cwd: CHECKOUT, stdout: "pipe", stderr: "pipe" },
  );
  if (gh.exitCode !== 0) {
    const err = new TextDecoder().decode(gh.stderr).trim();
    console.error(`from-issue: gh failed: ${err || "exit " + gh.exitCode}`);
    return 1;
  }

  const issue = JSON.parse(new TextDecoder().decode(gh.stdout)) as {
    title: string;
    body: string;
    number: number;
  };

  const data = await load();
  const feats = data.features ?? [];
  const existing = feats.find((f) => f.source_issue === issue.number);
  if (existing) {
    console.log(`issue #${issue.number} already registered as feature ${existing.id} (${existing.slug})`);
    console.log(resumeHint(existing));
    return 0;
  }

  const id = feats.reduce((m, f) => Math.max(m, f.id), 0) + 1;
  const slug = issue.title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 48) || `issue-${issue.number}`;

  const feature: Feature = {
    id,
    slug,
    title: issue.title,
    spec_raw: issue.body?.trim() || issue.title,
    status: "pending",
    review_rounds: 0,
    gate_attempts: 0,
    source_issue: issue.number,
    created_at: new Date().toISOString().slice(0, 10),
    acceptance: [],
  };
  feats.push(feature);
  data.features = feats;
  await saveFeatures(data);

  appendEvent(id, {
    type: "feature_from_issue",
    issue: issue.number,
    title: issue.title,
  });
  appendEvent(id, { type: "feature_registered", slug, title: issue.title });

  console.log(`registered feature ${id} (${slug}) from issue #${issue.number}`);
  console.log(`spec_raw stored — run /feature flow or launch spec-author for id ${id}`);
  console.log(nextAction(feats));
  return 0;
}

async function cmdShow(rawId: string | undefined): Promise<number> {
  const id = Number(rawId);
  if (rawId === undefined || !Number.isInteger(id)) {
    console.error(`invalid argument for 'show': expected an integer id, not '${rawId ?? ""}'`);
    return 2;
  }
  const data = await load();
  const match = (data.features ?? []).find((f) => f.id === id);
  if (match === undefined) {
    console.error(`there is no feature with id=${id}`);
    return 1;
  }
  console.log(JSON.stringify(match, null, 2));
  const orch = match.orchestrator_session_id;
  console.log();
  if (orch !== undefined && orch !== null) {
    console.log(`orchestrator_session_id: ${orch}`);
  } else {
    console.log("orchestrator_session_id: (not linked yet)");
  }

  // Every run of every step, in order. The counters (`review_rounds`,
  // `gate_attempts`) say how many; this says which provider ran what, when, and
  // how it ended — which is the question asked of a feature that went wrong.
  const attempts = match.attempts ?? [];
  if (attempts.length > 0) {
    console.log();
    console.log("attempts:");
    for (const a of attempts) {
      const who = a.provider ?? "?";
      const outcome = a.outcome ?? "running";
      const detail = a.detail === undefined ? "" : ` — ${a.detail}`;
      console.log(`  ${a.step} #${a.n}  ${who}  ${a.started_at}  ${outcome}${detail}`);
    }
  }

  const spec = join(SPECS, `${match.id}-${match.slug}`);
  console.log();
  console.log(`spec:     ${rel(spec)}${isDir(spec) ? "  (present)" : "  (missing)"}`);
  for (const [label, name] of [
    ["impl", "impl"],
    ["review", "review"],
    ["gate", "gate"],
    ["context", "context"],
    ["events", "events"],
  ] as const) {
    const path =
      name === "gate"
        ? gatePath(id)
        : name === "context"
          ? contextPath(id)
          : name === "events"
            ? eventsPath(id)
            : join(PROGRESS, `${name}_${id}.md`);
    const suffix = isFile(path) ? "  (present)" : "  (missing)";
    const display = name === "events" ? rel(eventsPath(id)) : rel(path);
    console.log(`${padEnd(`${label}:`, 10)}${display}${suffix}`);
  }

  const events = readEvents(id);
  if (events.length > 0) {
    const last = events[events.length - 1]!;
    console.log(`last event: ${last.type} @ ${last.ts}`);
  }
  console.log();
  console.log(resumeHint(match));
  return 0;
}

/** Latest open feature, or the newest row when everything is closed. */
function defaultFeatureId(
  feats: { id: number; status: string }[],
  rawId: string | undefined,
): number | undefined {
  if (rawId !== undefined) return Number(rawId);
  const open = feats
    .filter((f) => f.status !== "done")
    .sort((a, b) => b.id - a.id)[0];
  if (open) return open.id;
  return feats.length > 0 ? feats[feats.length - 1]!.id : undefined;
}

async function cmdDoctor(rawId: string | undefined): Promise<number> {
  const data = await load();
  const feats = data.features ?? [];
  console.log(`root:     ${ROOT}`);
  console.log(`harness:  ${rel(HARNESS)}`);
  // Worth saying out loud only when they differ: from a worktree the state
  // being edited is the main checkout's, not the one you are standing in.
  if (CHECKOUT !== ROOT) console.log(`checkout: ${CHECKOUT} (state shared from ${ROOT})`);
  console.log(`features: ${isFile(FEATURES) ? rel(FEATURES) : "MISSING — run ./init.sh"}`);
  console.log(`count:    ${feats.length} feature(s)`);

  const pick = defaultFeatureId(feats, rawId);
  if (pick === undefined || !Number.isInteger(pick)) {
    console.log("\n(no features — register with the Feature tab or /feature)");
    return 0;
  }
  const f = feats.find((x) => x.id === pick);
  if (f === undefined) {
    console.error(`there is no feature with id=${pick}`);
    return 1;
  }

  console.log();
  console.log(`feature ${f.id} (${f.slug}): ${f.status}`);
  if (rawId === undefined && f.status === "done" && feats.some((x) => x.status !== "done")) {
    console.log("  (default picks the latest open feature; pass an id to inspect a closed one)");
  } else if (rawId === undefined && feats.every((x) => x.status === "done")) {
    console.log("  (all features are closed — register a new one in Forge or pass an id)");
  }
  const orch = f.orchestrator_session_id;
  if (orch !== undefined && orch !== null && String(orch).length > 0) {
    console.log(`orchestrator_session_id: ${orch}`);
    console.log("  → should appear in Forge History when the daemon is running");
  } else {
    console.log("orchestrator_session_id: (not linked)");
    console.log("  → Forge has not launched the orchestrator PTY yet, or link failed");
  }

  const events = readEvents(f.id);
  console.log(`events:   ${events.length} line(s) in ${rel(eventsPath(f.id))}`);
  if (events.length > 0) {
    const last = events[events.length - 1]!;
    console.log(`last:     ${last.type} @ ${last.ts}`);
  }
  console.log();
  console.log(resumeHint(f));
  console.log();
  console.log("outside Forge:");
  console.log("  scripts/dev daemon info     # socket + paths");
  console.log(`  scripts/harness timeline ${f.id}  # full event log`);
  return 0;
}

async function cmdWatch(rawId: string | undefined): Promise<number> {
  const id = defaultFeatureId((await load()).features ?? [], rawId);
  if (rawId !== undefined && !Number.isInteger(id)) {
    console.error(`invalid id '${rawId}'`);
    return 2;
  }
  console.log(`watching feature ${id ?? "(latest)"} every 2s — Ctrl+C to stop\n`);
  for (;;) {
    await cmdDoctor(id !== undefined ? String(id) : undefined);
    await Bun.sleep(2000);
  }
}

async function cmdValidate(): Promise<number> {
  return await validate();
}

function cmdGate(fast: boolean): number {
  // The gate compiles and tests the code, which lives in *this* checkout —
  // only the harness state is shared with the project's other worktrees.
  const cmd = [join(CHECKOUT, "init.sh")];
  if (fast) cmd.push("--fast");
  const proc = Bun.spawnSync(cmd, { cwd: CHECKOUT, stdio: ["inherit", "inherit", "inherit"] });
  return proc.exitCode ?? 1;
}

const USAGE = `usage: scripts/harness [-h] {status,list,next,active,root,show,doctor,watch,resume,timeline,event,from-issue,validate,gate} ...

Interface of the subagent harness (state on disk).

commands:
  status            glance: features + next action (default)
  list              table of features
  next              one line: what to do now
  active            \`id|slug|status\` of the active feature (empty if none)
  root              absolute path of the checkout that owns harness/ (worktree-safe)
  show <id>         JSON detail + artefact map for one feature
  doctor [id]       diagnose one feature (default: latest) — orchestrator link, events
  watch [id]        poll doctor every 2s (Ctrl+C to stop)
  resume <id>       where a feature left off + suggested command
  timeline <id>     append-only event log for one feature
  event <id> <type> [--data '{...}']  append one harness event
  from-issue <n>    register a feature from a GitHub issue (needs gh)
  validate          coherence of features.json
  gate [--fast]     runs ./init.sh

options:
  -h, --help        show this help and exit`;

async function main(argv: string[]): Promise<number> {
  if (argv.includes("-h") || argv.includes("--help")) {
    console.log(USAGE);
    return 0;
  }

  const cmd = argv[0] ?? "status";
  const rest = argv.slice(1);

  switch (cmd) {
    case "status":
      return await cmdStatus();
    case "list":
      return await cmdList();
    case "next":
      return await cmdNext();
    case "active":
      return await cmdActive();
    case "root":
      return cmdRoot();
    case "resume":
      return await cmdResume(rest[0]);
    case "timeline":
      return await cmdTimeline(rest[0]);
    case "event":
      return await cmdEvent(rest);
    case "from-issue":
      return await cmdFromIssue(rest[0]);
    case "show":
      return await cmdShow(rest[0]);
    case "doctor":
      return await cmdDoctor(rest[0]);
    case "watch":
      return await cmdWatch(rest[0]);
    case "validate":
      return await cmdValidate();
    case "gate":
      return cmdGate(rest.includes("--fast"));
    default:
      console.error(`unknown command: '${cmd}'`);
      console.error(USAGE);
      return 2;
  }
}

if (import.meta.main) {
  process.exit(await main(Bun.argv.slice(2)));
}

export { resumeHint, nextAction };
