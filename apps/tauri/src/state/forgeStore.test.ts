import { createComputed, createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { sessionFixture } from "../contracts/sessions.fixture";
import type { Launchable, ShellSnapshot } from "../contracts/runtime";
import { applyShellSnapshot, emptySnapshot, forgeStore } from "./forgeStore";

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
