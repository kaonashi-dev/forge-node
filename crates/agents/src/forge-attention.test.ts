import { afterEach, expect, mock, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

mock.module("@opencode/plugin/tui", () => ({
  Plugin: { define: <T>(definition: T): T => definition },
}));
const { activityReporter, observeActivity } = await import("./forge-attention");
type ActivityContext = import("./forge-attention").ActivityContext;
type Activity = import("./forge-attention").Activity;

type Listener = (event: { data: Record<string, unknown> }) => void;

/** A TUI showing `route`, whose sessions are their own roots unless `roots` says otherwise. */
function fakeTui(roots: Record<string, string> = {}) {
  const listeners = new Map<string, Listener>();
  let route: { type: string; sessionID?: string } = { type: "home" };
  const context = {
    ui: { router: { current: () => route } },
    data: {
      session: { root: (id: string) => roots[id] ?? id },
      on: (event: string, listener: Listener) => {
        listeners.set(event, listener);
        return () => listeners.delete(event);
      },
    },
  } as unknown as ActivityContext;
  return {
    context,
    show(sessionID?: string) {
      route = sessionID ? { type: "session", sessionID } : { type: "home" };
    },
    emit(event: string, data: Record<string, unknown>) {
      listeners.get(event)?.({ data });
    },
    listening: () => listeners.size,
  };
}

test("only the session this TUI shows is reported, once per change", () => {
  const tui = fakeTui({ child: "mine" });
  const seen: Activity[] = [];
  const stop = observeActivity(tui.context, (state) => seen.push(state));

  tui.emit("session.execution.started", { sessionID: "mine" });
  expect(seen).toEqual([]);

  tui.show("mine");
  tui.emit("session.execution.started", { sessionID: "mine" });
  tui.emit("session.execution.started", { sessionID: "mine" });
  tui.emit("session.execution.started", { sessionID: "other" });
  tui.emit("session.execution.started", { sessionID: "child" });
  tui.emit("permission.asked", { sessionID: "child" });
  tui.emit("form.created", { form: { sessionID: "mine" } });
  expect(seen).toEqual(["working", "waiting"]);

  tui.show();
  tui.emit("session.execution.succeeded", { sessionID: "mine" });
  expect(seen).toEqual(["working", "waiting", "idle"]);

  stop();
  expect(tui.listening()).toBe(0);
});

const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

test("a state reported while one runs replaces any still waiting", async () => {
  const dir = mkdtempSync(join(tmpdir(), "forge-attention-"));
  dirs.push(dir);
  const log = join(dir, "log");
  const script = join(dir, "activity.sh");
  writeFileSync(script, `#!/bin/sh\necho "$1 $2" >> '${log}'\nsleep 0.2\n`);
  chmodSync(script, 0o700);

  const reporter = activityReporter(script);
  reporter.report("working");
  reporter.report("waiting");
  reporter.report("idle");
  await Bun.sleep(700);
  reporter.close();

  expect(readFileSync(log, "utf8").trim().split("\n")).toEqual([
    "working opencode",
    "idle opencode",
  ]);
});
