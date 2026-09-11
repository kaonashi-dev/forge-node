/**
 * Harness tests. They run the CLI and the validator as subprocesses against
 * fake repos in tmp, so the disk paths are covered too.
 */

import { afterEach, describe, expect, test } from "bun:test";
import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

const REPO = join(import.meta.dir, "..", "..");
const CLI = join(REPO, "harness", "src", "cli.ts");
const VALIDATE = join(REPO, "harness", "src", "validate.ts");
const EVENTS = join(REPO, "harness", "src", "events.ts");

const temps: string[] = [];

afterEach(() => {
  for (const d of temps.splice(0)) rmSync(d, { recursive: true, force: true });
});

interface FakeFeature {
  id: number;
  slug: string;
  status: string;
  review_rounds?: number;
  gate_attempts?: number;
  attempts?: Record<string, unknown>[];
  blocked_reason?: string;
}

/**
 * Builds a fake repo: harness/{features.json,specs/,progress/} plus a copy of
 * src/, so that ROOT (= two levels above the module) points there.
 */
function fakeRepo(opts: {
  features: FakeFeature[];
  rules?: Record<string, unknown>;
  specs?: string[];
  reviews?: Record<number, string>;
  gates?: number[];
  contexts?: number[];
  eventLogs?: Record<number, string>;
  raw?: string;
}): { root: string; cli: string; validate: string } {
  const root = mkdtempSync(join(tmpdir(), "forge-harness-"));
  temps.push(root);
  const h = join(root, "harness");
  mkdirSync(join(h, "src"), { recursive: true });
  mkdirSync(join(h, "specs"), { recursive: true });
  mkdirSync(join(h, "progress"), { recursive: true });

  for (const name of ["harness.ts", "cli.ts", "validate.ts", "events.ts"]) {
    copyFileSync(join(REPO, "harness", "src", name), join(h, "src", name));
  }

  const body =
    opts.raw ??
    JSON.stringify(
      {
        project: "fake",
        rules: {
          max_review_rounds: 2,
          max_gate_attempts: 3,
          require_gate_file: true,
          require_context_bundle: true,
          valid_status: [
            "pending",
            "spec_ready",
            "in_progress",
            "in_review",
            "done",
            "blocked",
          ],
          ...(opts.rules ?? {}),
        },
        features: opts.features,
      },
      null,
      2,
    );
  writeFileSync(join(h, "features.json"), body);

  for (const s of opts.specs ?? []) {
    const dir = join(h, "specs", s);
    mkdirSync(dir, { recursive: true });
    for (const f of ["requirements.md", "design.md", "tasks.md"]) {
      writeFileSync(join(dir, f), "# stub\n");
    }
  }
  for (const [id, text] of Object.entries(opts.reviews ?? {})) {
    writeFileSync(join(h, "progress", `review_${id}.md`), text);
  }
  for (const id of opts.gates ?? []) {
    writeFileSync(join(h, "progress", `gate_${id}.md`), "# gate\n");
  }
  for (const id of opts.contexts ?? []) {
    writeFileSync(join(h, "progress", `context_${id}.md`), "# context\n");
  }
  for (const [id, text] of Object.entries(opts.eventLogs ?? {})) {
    writeFileSync(join(h, "progress", `events_${id}.jsonl`), text);
  }

  return { root, cli: join(h, "src", "cli.ts"), validate: join(h, "src", "validate.ts") };
}

function run(script: string, args: string[] = [], cwd = REPO) {
  // A session started by the Forge daemon exports FORGE_HARNESS_ROOT, and
  // `harnessRoot()` honours it before anything else — inherited here it would
  // point every fake repo at the real checkout, so the suite would both fail
  // and write into it.
  const { FORGE_HARNESS_ROOT: _ignored, ...env } = process.env;
  const p = Bun.spawnSync(["bun", script, ...args], { cwd, env, stdout: "pipe", stderr: "pipe" });
  return {
    code: p.exitCode,
    out: new TextDecoder().decode(p.stdout),
    err: new TextDecoder().decode(p.stderr),
  };
}

describe("validate", () => {
  test("accepts a coherent state", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "a-thing", status: "done", review_rounds: 1 }],
      specs: ["1-a-thing"],
      reviews: { 1: "verdict: APPROVED\n" },
      gates: [1],
      contexts: [1],
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"review_verdict","verdict":"APPROVED"}\n',
      },
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(0);
    expect(out).toContain("[OK]");
    expect(out).toContain("done: 1");
  });

  test("rejects two features active at once", () => {
    const r = fakeRepo({
      features: [
        { id: 1, slug: "one", status: "in_progress" },
        { id: 2, slug: "another", status: "in_review" },
      ],
      specs: ["1-one", "2-another"],
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("2 features active at once");
  });

  test("rejects duplicate ids", () => {
    const r = fakeRepo({
      features: [
        { id: 1, slug: "one", status: "pending" },
        { id: 1, slug: "another", status: "pending" },
      ],
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("duplicate id: 1");
  });

  test("rejects a non kebab-case slug", () => {
    const r = fakeRepo({ features: [{ id: 1, slug: "A_Thing", status: "pending" }] });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("slug missing or not kebab-case");
  });

  test("rejects an invalid status", () => {
    const r = fakeRepo({ features: [{ id: 1, slug: "one", status: "made-up" }] });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("invalid status 'made-up'");
  });

  test("requires the spec folder from spec_ready on", () => {
    const r = fakeRepo({ features: [{ id: 1, slug: "one", status: "spec_ready" }] });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("harness/specs/1-one/{requirements.md,design.md,tasks.md}");
  });

  test("requires gate file from spec_ready on", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "spec_ready" }],
      specs: ["1-one"],
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("harness/progress/gate_1.md is missing");
  });

  test("requires context bundle from in_progress on", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "in_progress" }],
      specs: ["1-one"],
      gates: [1],
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("harness/progress/context_1.md is missing");
  });

  test("rejects gate_attempts above the maximum", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "blocked", gate_attempts: 4 }],
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("gate_attempts=4 exceeds max_gate_attempts=3");
  });

  test("requires an APPROVED review for done", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "done" }],
      specs: ["1-one"],
      gates: [1],
      contexts: [1],
      reviews: { 1: "verdict: REJECTED\n" },
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"review_verdict","verdict":"APPROVED"}\n',
      },
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("no line that is exactly APPROVED");
  });

  // The check used to be `text.includes("APPROVED")`, and the reviewer's own
  // format example in `.claude/agents/reviewer.md` prints that word. A review
  // left on the template marked the feature done without anyone reviewing it.
  test("does not accept the unfilled verdict template as an approval", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "done" }],
      specs: ["1-one"],
      gates: [1],
      contexts: [1],
      reviews: { 1: "# Review\n\n**Verdict:** APPROVED | CHANGES_REQUESTED\n" },
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"review_verdict","verdict":"APPROVED"}\n',
      },
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("no line that is exactly APPROVED");
  });

  test("does not accept prose that argues against approving", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "done" }],
      specs: ["1-one"],
      gates: [1],
      contexts: [1],
      reviews: { 1: "This cannot be APPROVED: C4 is empty.\n" },
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"review_verdict","verdict":"APPROVED"}\n',
      },
    });
    expect(run(r.validate).code).toBe(1);
  });

  test("accepts a review whose verdict is on its own line", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "done" }],
      specs: ["1-one"],
      gates: [1],
      contexts: [1],
      reviews: { 1: "# Review\n\nAll checkpoints pass.\n\n**Verdict:** APPROVED\n" },
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"review_verdict","verdict":"APPROVED"}\n',
      },
    });
    expect(run(r.validate).code).toBe(0);
  });

  test("requires the review of a done feature to exist", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "done" }],
      specs: ["1-one"],
      gates: [1],
      contexts: [1],
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"review_verdict","verdict":"APPROVED"}\n',
      },
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("'done' without harness/progress/review_1.md");
  });

  test("rejects review_rounds above the maximum", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "in_review", review_rounds: 5 }],
      specs: ["1-one"],
      gates: [1],
      contexts: [1],
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"impl_done"}\n',
      },
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("review_rounds=5 exceeds max_review_rounds=2");
  });

  test("rejects invalid JSON", () => {
    const r = fakeRepo({ features: [], raw: "{ this is not json" });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("is not valid JSON");
  });

  test("rejects an empty valid_status", () => {
    const r = fakeRepo({
      features: [],
      raw: JSON.stringify({ rules: { valid_status: [] }, features: [] }),
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("rules.valid_status empty or missing");
  });

  test("accepts a harness with no features", () => {
    const r = fakeRepo({ features: [] });
    const { code, out } = run(r.validate);
    expect(code).toBe(0);
    expect(out).toContain("no features");
  });

  // features.json is gitignored, so this is what every fresh clone looks like.
  test("accepts a checkout with no features.json yet", () => {
    const r = fakeRepo({ features: [] });
    rmSync(join(r.root, "harness", "features.json"));
    const { code, out } = run(r.validate);
    expect(code).toBe(0);
    expect(out).toContain("no harness state in this checkout");
  });

  // The spec step runs under `pending`, so a live attempt there is a step in
  // flight — not a leftover. Flagging it turned every spec run into an
  // "incoherent" harness and had the Stop hook refuse to close sessions.
  test("a spec step in flight is not a stale attempt", () => {
    const r = fakeRepo({
      features: [
        {
          id: 1,
          slug: "running",
          status: "pending",
          attempts: [{ step: "Spec", n: 1, started_at: "2026-01-01T00:00:00Z" }],
        },
      ],
    });
    expect(run(r.validate).code).toBe(0);
  });

  test("a live attempt on a feature at rest is flagged", () => {
    const r = fakeRepo({
      features: [
        {
          id: 1,
          slug: "leftover",
          status: "blocked",
          blocked_reason: "x",
          attempts: [{ step: "Implement", n: 1, started_at: "2026-01-01T00:00:00Z" }],
        },
      ],
    });
    const { code, out } = run(r.validate);
    expect(code).toBe(1);
    expect(out).toContain("still recorded as running");
  });

  // `max_step_attempts` is the daemon's budget for automatic restarts; a human
  // `RetryStep` is a legal attempt past it and must not make the state invalid.
  test("attempts past max_step_attempts are the daemon's business, not an error", () => {
    const settled = (n: number) => ({
      step: "Implement",
      n,
      started_at: "2026-01-01T00:00:00Z",
      settled_at: "2026-01-01T00:00:00Z",
      outcome: "failed",
    });
    const r = fakeRepo({
      rules: { max_step_attempts: 2 },
      features: [
        {
          id: 1,
          slug: "retried",
          status: "blocked",
          blocked_reason: "spent",
          attempts: [settled(1), settled(2), settled(3)],
        },
      ],
    });
    expect(run(r.validate).code).toBe(0);
  });
});

describe("cli", () => {
  test("list aligns columns and flags missing specs", () => {
    const r = fakeRepo({ features: [{ id: 7, slug: "no-spec", status: "spec_ready" }] });
    const { code, out } = run(r.cli, ["list"]);
    expect(code).toBe(0);
    expect(out).toContain("  7  no-spec");
    expect(out).toContain("⚠ spec missing");
    expect(out).toContain("⚠ gate missing");
    expect(out).toContain("/feature-go 7");
  });

  test("list with no features suggests registering one", () => {
    const r = fakeRepo({ features: [] });
    expect(run(r.cli, ["list"]).out).toContain("(no features");
  });

  test("next prioritises the active feature over the rest", () => {
    const r = fakeRepo({
      features: [
        { id: 1, slug: "ready", status: "spec_ready" },
        { id: 2, slug: "in-flight", status: "in_progress" },
      ],
      specs: ["1-ready", "2-in-flight"],
    });
    // "in that checkout": the slot a feature occupies is its checkout, so the
    // sentence names the scope of the restriction, not the whole project.
    expect(run(r.cli, ["next"]).out.trim()).toBe(
      "next: feature 2 (in-flight) is in_progress — do not open another one in that checkout",
    );
  });

  test("next points at blocked ones before ready ones", () => {
    const r = fakeRepo({
      features: [
        { id: 1, slug: "ready", status: "spec_ready" },
        { id: 2, slug: "stuck", status: "blocked" },
      ],
      specs: ["1-ready"],
    });
    expect(run(r.cli, ["next"]).out).toContain("unblock feature 2 (stuck)");
  });

  test("next falls back to /feature when there is nothing", () => {
    const r = fakeRepo({ features: [] });
    expect(run(r.cli, ["next"]).out).toContain('/feature "<specification>"');
  });

  test("active prints id|slug|status of the in-flight feature", () => {
    const r = fakeRepo({
      features: [
        { id: 1, slug: "closed", status: "done" },
        { id: 2, slug: "in-flight", status: "in_review" },
      ],
      specs: ["1-closed", "2-in-flight"],
      reviews: { 1: "APPROVED" },
    });
    // Fourth field is `workspace_path`, empty for a feature registered outside
    // Forge — the session guard splits on it, so the trailing `|` is load-bearing.
    expect(run(r.cli, ["active"]).out.trim()).toBe("2|in-flight|in_review|");
  });

  test("active prints nothing with no active feature", () => {
    const r = fakeRepo({ features: [{ id: 1, slug: "one", status: "pending" }] });
    const { code, out } = run(r.cli, ["active"]);
    expect(code).toBe(0);
    expect(out.trim()).toBe("");
  });

  test("show returns the feature JSON and the file map", () => {
    const r = fakeRepo({
      features: [{ id: 3, slug: "three", status: "spec_ready" }],
      specs: ["3-three"],
      gates: [3],
    });
    const { code, out } = run(r.cli, ["show", "3"]);
    expect(code).toBe(0);
    expect(JSON.parse(out.slice(0, out.indexOf("\n\n"))).slug).toBe("three");
    expect(out).toContain("harness/specs/3-three  (present)");
    expect(out).toContain("gate:     harness/progress/gate_3.md  (present)");
    expect(out).toContain("impl:     harness/progress/impl_3.md  (missing)");
  });

  test("show with a non-existent id exits with 1 and writes to stderr", () => {
    const r = fakeRepo({ features: [] });
    const { code, out, err } = run(r.cli, ["show", "42"]);
    expect(code).toBe(1);
    expect(err).toContain("there is no feature with id=42");
    expect(out).toBe("");
  });

  test("show without an id exits with 2", () => {
    const r = fakeRepo({ features: [] });
    const { code, err } = run(r.cli, ["show"]);
    expect(code).toBe(2);
    expect(err).toContain("expected an integer id");
  });

  test("status is the default command", () => {
    const r = fakeRepo({ features: [{ id: 1, slug: "one", status: "pending" }] });
    const bare = run(r.cli, []);
    expect(bare.code).toBe(0);
    expect(bare.out).toContain("project: fake");
    expect(bare.out).toBe(run(r.cli, ["status"]).out);
  });

  test("status summarises the counts per status", () => {
    const r = fakeRepo({
      features: [
        { id: 1, slug: "one", status: "pending" },
        { id: 2, slug: "two", status: "blocked" },
      ],
    });
    expect(run(r.cli, ["status"]).out).toContain(
      "features: 2  (pending=1, spec_ready=0, active=0, blocked=1)",
    );
  });

  test("validate from the cli mirrors the validator", () => {
    const r = fakeRepo({ features: [{ id: 1, slug: "one", status: "pending" }] });
    expect(run(r.cli, ["validate"]).out).toBe(run(r.validate).out);
  });

  test("--help exits with 0 and lists the commands", () => {
    const { code, out } = run(CLI, ["--help"]);
    expect(code).toBe(0);
    expect(out).toContain("usage: scripts/harness");
    expect(out).toContain("timeline");
    expect(out).toContain("from-issue");
    expect(out).toContain("gate [--fast]");
  });

  test("resume prints a hint for spec_ready", () => {
    const r = fakeRepo({
      features: [{ id: 5, slug: "five", status: "spec_ready" }],
      specs: ["5-five"],
      gates: [5],
    });
    const { code, out } = run(r.cli, ["resume", "5"]);
    expect(code).toBe(0);
    expect(out).toContain("/feature-go 5");
    expect(out).toContain("gate_5.md");
  });

  test("timeline prints event lines", () => {
    const r = fakeRepo({
      features: [{ id: 1, slug: "one", status: "pending" }],
      eventLogs: {
        1: '{"ts":"2026-01-01T00:00:00Z","type":"feature_registered","slug":"one"}\n',
      },
    });
    const { code, out } = run(r.cli, ["timeline", "1"]);
    expect(code).toBe(0);
    expect(out).toContain("feature_registered");
  });

  test("event appends to the jsonl log", () => {
    const r = fakeRepo({ features: [{ id: 9, slug: "nine", status: "pending" }] });
    const { code, out } = run(r.cli, ["event", "9", "feature_registered", "--data", '{"slug":"nine"}']);
    expect(code).toBe(0);
    expect(out).toContain("appended feature_registered");
    const timeline = run(r.cli, ["timeline", "9"]);
    expect(timeline.out).toContain("feature_registered");
  });

  test("an unknown command exits with 2", () => {
    const { code, err } = run(CLI, ["made-up"]);
    expect(code).toBe(2);
    expect(err).toContain("unknown command: 'made-up'");
  });
});

describe("root", () => {
  test("FORGE_HARNESS_ROOT wins over the checkout", () => {
    const r = fakeRepo({ features: [] });
    const p = Bun.spawnSync(["bun", r.cli, "root"], {
      cwd: REPO,
      env: { ...process.env, FORGE_HARNESS_ROOT: "/somewhere/else" },
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(p.exitCode).toBe(0);
    expect(new TextDecoder().decode(p.stdout).trim()).toBe("/somewhere/else");
  });

  // The regression this whole change exists for: an agent standing in a
  // worktree must be told the *main* checkout, not the one under its feet.
  test("from a worktree it answers the main checkout", () => {
    const git = (cwd: string, ...args: string[]) =>
      Bun.spawnSync(["git", ...args], { cwd, stdout: "pipe", stderr: "pipe" });
    if (Bun.spawnSync(["git", "--version"]).exitCode !== 0) return; // no git, no case

    const main = realpathSync(mkdtempSync(join(tmpdir(), "forge-root-")));
    temps.push(main);
    git(main, "init", "-q", "-b", "main");
    git(main, "config", "user.email", "t@example.com");
    git(main, "config", "user.name", "t");
    mkdirSync(join(main, "harness", "src"), { recursive: true });
    mkdirSync(join(main, "harness", "progress"), { recursive: true });
    mkdirSync(join(main, "harness", "specs"), { recursive: true });
    for (const name of ["harness.ts", "cli.ts", "validate.ts", "events.ts"]) {
      copyFileSync(join(REPO, "harness", "src", name), join(main, "harness", "src", name));
    }
    writeFileSync(join(main, "harness", "features.json"), '{"project":"t","features":[]}\n');
    git(main, "add", "-A");
    git(main, "commit", "-qm", "init");

    const wt = join(main, "..", `${basename(main)}-wt`);
    git(main, "worktree", "add", "-q", "-b", "side", wt);
    temps.push(wt);

    const { FORGE_HARNESS_ROOT: _ignored, ...env } = process.env;
    const p = Bun.spawnSync(["bun", join(wt, "harness", "src", "cli.ts"), "root"], {
      cwd: wt,
      env,
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(p.exitCode).toBe(0);
    expect(realpathSync(new TextDecoder().decode(p.stdout).trim())).toBe(main);
  });
});

describe("real repo", () => {
  test("the repo's harness/features.json is coherent", () => {
    expect(run(VALIDATE).code).toBe(0);
  });
});
