import { createComputed, createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { sessionFixture } from "../runtime/sessions.fixture";
import type { ShellSnapshot } from "../runtime/types";
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

describe("applyShellSnapshot", () => {
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
