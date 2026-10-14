import { createComputed, createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { sessionFixture } from "../contracts/sessions.fixture";
import type {
  ExternalAgentSession,
  Launchable,
  PullRequest,
  ShellSnapshot,
} from "../contracts/runtime";
import { applyShellSnapshot, emptySnapshot, forgeStore } from "./forgeStore";
import { loading, setLoading } from "./loading";

/**
 * A snapshot arrives from the IPC freshly parsed, so *every* object in it is a
 * new reference however little of it moved. These tests describe what
 * `applyShellSnapshot` has to do about that: a row that did not change must
 * keep its identity, or `<For>` rebuilds the whole rail on every daemon event.
 */
const snapshot = (sessions: ReturnType<typeof sessionFixture>[]): ShellSnapshot => ({
  ...emptySnapshot(),
  sessions,
  live_sessions: sessions.length,
});

const first = () => sessionFixture({ id: "a" });
const second = () => sessionFixture({ id: "b" });

const launchables: Launchable[] = [
  {
    kind: "shell",
    label: "New Terminal",
    detail: null,
    provider: null,
    profile: null,
    enabled: true,
    key: "shell",
    supports_initial_prompt: false,
  },
  {
    kind: "agent",
    label: "OpenCode",
    detail: null,
    provider: "opencode",
    profile: null,
    enabled: true,
    key: "opencode",
    supports_initial_prompt: true,
  },
  {
    kind: "agent",
    label: "Custom model",
    detail: null,
    provider: "opencode",
    profile: "custom-model",
    enabled: true,
    key: "profile:custom-model",
    supports_initial_prompt: true,
  },
];

describe("applyShellSnapshot", () => {
  it("hides a disabled provider from selectors while retaining its profiles and sessions", () => {
    const input: ShellSnapshot = {
      ...snapshot([sessionFixture({ agent_provider_id: "opencode" })]),
      providers: [{ descriptor: { id: "opencode", display_name: "OpenCode" } }],
      launchables,
      app_state: { "ui.agent_visible.opencode": "false" },
    };

    applyShellSnapshot(input);

    expect(forgeStore.launchables.map((item) => item.key)).toEqual([
      "shell",
      "profile:custom-model",
    ]);
    expect(forgeStore.providers).toEqual(input.providers);
    expect(forgeStore.sessions).toEqual(input.sessions);
    expect(input.launchables).toHaveLength(3);

    applyShellSnapshot({ ...input, app_state: { "ui.agent_visible.opencode": "true" } });
    expect(forgeStore.launchables).toEqual(launchables);
  });

  it("hides profiles independently and keeps a terminal when every agent is disabled", () => {
    applyShellSnapshot({
      ...emptySnapshot(),
      launchables,
      app_state: { "ui.agent_visible.profile:custom-model": "false" },
    });
    expect(forgeStore.launchables.map((item) => item.key)).toEqual(["shell", "opencode"]);

    applyShellSnapshot({
      ...emptySnapshot(),
      launchables,
      app_state: {
        "ui.agent_visible.opencode": "false",
        "ui.agent_visible.profile:custom-model": "false",
      },
    });
    expect(forgeStore.launchables.map((item) => item.key)).toEqual(["shell"]);
  });

  it("keeps the identity of a row that did not change", () => {
    applyShellSnapshot(snapshot([first(), second()]));
    const before = forgeStore.sessions[0];

    // Same content, all-new objects: what the next daemon event looks like.
    applyShellSnapshot(snapshot([first(), second()]));

    expect(forgeStore.sessions[0]).toBe(before);
  });

  it("notifies nobody when the snapshot changed nothing", () => {
    applyShellSnapshot(snapshot([first(), second()]));
    let runs = 0;
    const dispose = createRoot((dispose) => {
      // `createComputed` rather than `createEffect`: it runs synchronously, so
      // the count is settled by the time `createRoot` returns and the test
      // does not depend on when a queue is flushed.
      createComputed(() => {
        forgeStore.sessions.map((session) => session.state);
        runs += 1;
      });
      return dispose;
    });
    expect(runs).toBe(1);

    applyShellSnapshot(snapshot([first(), second()]));
    expect(runs).toBe(1);

    // ...and it still notifies when something did move.
    applyShellSnapshot(snapshot([sessionFixture({ id: "a", state: "Exited" }), second()]));
    expect(runs).toBe(2);
    dispose();
  });

  it("writes only the leaf that moved", () => {
    applyShellSnapshot(snapshot([first(), second()]));
    const untouched = forgeStore.sessions[1];

    applyShellSnapshot(snapshot([sessionFixture({ id: "a", state: "Exited" }), second()]));

    expect(forgeStore.sessions[0].state).toBe("Exited");
    expect(forgeStore.sessions[1]).toBe(untouched);
  });

  it("still replaces a row whose id changed", () => {
    applyShellSnapshot(snapshot([first()]));
    applyShellSnapshot(snapshot([second()]));

    expect(forgeStore.sessions).toHaveLength(1);
    expect(forgeStore.sessions[0].id).toBe("b");
  });

  it("drops rows the snapshot no longer carries", () => {
    applyShellSnapshot(snapshot([first(), second()]));
    applyShellSnapshot(snapshot([first()]));

    expect(forgeStore.sessions.map((session) => session.id)).toEqual(["a"]);
  });
});

describe("rows keyed by something other than an id", () => {
  const run = (session_id: string): ExternalAgentSession => ({
    session_id,
    project_id: "p-1",
    workspace_id: "w-1",
    provider: "claude",
    profile_id: null,
    title: session_id,
    branch: null,
    preview: null,
    model: null,
    message_count: 1,
    subagent_count: 0,
    transcript_path: `/tmp/${session_id}.jsonl`,
    store: "File",
    started_at: "2026-01-01T00:00:00Z",
    last_activity: "2026-01-01T00:00:00Z",
  });
  const pr = (number: number, updated_at = "2026-01-01T00:00:00Z"): PullRequest => ({
    project_id: "p1",
    repository: "acme/widget",
    host: "github.com",
    number,
    title: `#${number}`,
    body: "",
    body_truncated: false,
    url: `https://github.com/acme/widget/pull/${number}`,
    author: "rin",
    base_ref: "main",
    head_ref: "topic",
    is_draft: false,
    review_decision: null,
    labels: [],
    assignees: [],
    review_requests: [],
    additions: 0,
    deletions: 0,
    changed_files: 0,
    comment_count: 0,
    created_at: "2026-01-01T00:00:00Z",
    updated_at,
    relations: { assigned: false, review_requested: false, authored: false },
  });
  const withRows = (runs: ExternalAgentSession[], prs: PullRequest[]): ShellSnapshot => ({
    ...emptySnapshot(),
    external_agents: runs,
    pull_requests: { ...emptySnapshot().pull_requests, pull_requests: prs },
  });

  // A new transcript sorts first; matched by position, it would rewrite every
  // card below it.
  it("keeps a discovered run's identity when another lands above it", () => {
    applyShellSnapshot(withRows([run("old")], []));
    const before = forgeStore.external_agents[0];

    applyShellSnapshot(withRows([run("new"), run("old")], []));

    expect(forgeStore.external_agents.map((item) => item.session_id)).toEqual(["new", "old"]);
    expect(forgeStore.external_agents[1]).toBe(before);
  });

  it("keeps a pull request's identity when the list re-sorts", () => {
    applyShellSnapshot(withRows([], [pr(1), pr(2)]));
    const second = forgeStore.pull_requests.pull_requests[1];

    applyShellSnapshot(withRows([], [pr(2, "2026-01-02T00:00:00Z"), pr(1)]));

    expect(forgeStore.pull_requests.pull_requests[0]).toBe(second);
    expect(second.updated_at).toBe("2026-01-02T00:00:00Z");
  });
});

describe("a pull-request refresh", () => {
  const withRefresh = (refreshed_at: string | null, error: string | null = null) => ({
    ...emptySnapshot(),
    pull_requests: { ...emptySnapshot().pull_requests, refreshed_at, error },
  });

  it("is over when a snapshot carries a newer answer", () => {
    applyShellSnapshot(withRefresh("2026-09-28T10:00:00Z"));
    setLoading("pull_requests", true);

    applyShellSnapshot(withRefresh("2026-09-28T10:01:00Z"));

    expect(loading.pull_requests).toBe(false);
  });

  it("is over when the answer is an error and the timestamp did not move", () => {
    applyShellSnapshot(withRefresh("2026-09-28T10:00:00Z"));
    setLoading("pull_requests", true);

    applyShellSnapshot(withRefresh("2026-09-28T10:00:00Z", "gh is not signed in"));

    expect(loading.pull_requests).toBe(false);
  });

  it("stays in flight through a snapshot that says nothing about it", () => {
    applyShellSnapshot(withRefresh("2026-09-28T10:00:00Z"));
    setLoading("pull_requests", true);

    applyShellSnapshot(withRefresh("2026-09-28T10:00:00Z"));

    expect(loading.pull_requests).toBe(true);
  });
});
