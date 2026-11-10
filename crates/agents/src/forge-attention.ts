import { spawn, type ChildProcess } from "node:child_process";
import { Plugin, type PluginContext } from "@opencode/plugin/tui";

export type Activity = "working" | "waiting" | "idle";

/** The part of OpenCode's TUI plugin context this plugin reads. */
export type ActivityContext = Pick<PluginContext, "ui" | "data">;

export interface Reporter {
  report(state: Activity): void;
  close(): void;
}

export function observeActivity(
  context: ActivityContext,
  report: (state: Activity) => void,
): () => void {
  let previous: string | undefined;
  // The server is shared by every OpenCode TUI, so only this TUI's session
  // counts; it stays this TUI's after a return home, or that turn's idle
  // would be dropped and the session left working.
  let shown: string | undefined;
  const publish = (sessionID: string, state: Activity, rootOnly = false): void => {
    const route = context.ui.router.current();
    if (route.type === "session" && "sessionID" in route) {
      shown = context.data.session.root(route.sessionID);
    }
    if (shown === undefined) return;
    const id = shown;
    if (context.data.session.root(sessionID) !== id || (rootOnly && sessionID !== id)) return;
    const key = `${id}:${state}`;
    if (key === previous) return;
    previous = key;
    report(state);
  };
  const stops = [
    context.data.on("permission.asked", (e) => publish(e.data.sessionID, "waiting")),
    context.data.on("permission.replied", (e) => publish(e.data.sessionID, "working")),
    context.data.on("form.created", (e) => publish(e.data.form.sessionID, "waiting")),
    context.data.on("form.replied", (e) => publish(e.data.sessionID, "working")),
    context.data.on("form.cancelled", (e) => publish(e.data.sessionID, "working")),
    context.data.on("session.execution.started", (e) =>
      publish(e.data.sessionID, "working", true),
    ),
    ...[
      "session.execution.succeeded",
      "session.execution.failed",
      "session.execution.interrupted",
    ].map((event) => context.data.on(event, (e) => publish(e.data.sessionID, "idle", true))),
  ];
  return () => stops.forEach((stop) => stop());
}

/** At most one `activity` process at a time; a newer state replaces one still waiting. */
export function activityReporter(activity: string, killAfterMs = 1500): Reporter {
  let child: ChildProcess | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pending: Activity | undefined;
  let closed = false;
  const kill = (): void => {
    if (!child?.pid) return;
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      child.kill("SIGKILL");
    }
  };
  const drain = (): void => {
    if (closed || child || !pending) return;
    const state = pending;
    pending = undefined;
    child = spawn(activity, [state, "opencode"], { detached: true, stdio: "ignore" });
    timer = setTimeout(kill, killAfterMs);
    const done = (): void => {
      clearTimeout(timer);
      child = undefined;
      drain();
    };
    child.once("error", () => {});
    child.once("close", done);
  };
  return {
    report(state) {
      if (closed) return;
      pending = state;
      drain();
    },
    close() {
      closed = true;
      pending = undefined;
      clearTimeout(timer);
      kill();
    },
  };
}

export default Plugin.define({
  id: "forge.activity",
  setup(context) {
    const activity = context.options.activity;
    if (!process.env.FORGE_SESSION_ID || typeof activity !== "string" || !activity) return;
    const reporter = activityReporter(activity);
    const dispose = observeActivity(context, reporter.report);
    return () => {
      dispose();
      reporter.close();
    };
  },
});
