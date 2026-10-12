import { spawn } from "node:child_process";
import { Plugin } from "@opencode/plugin/tui";

export function observeActivity(context, report) {
  let previous;
  const publish = (sessionID, state, rootOnly = false) => {
    const route = context.ui.router.current();
    if (route.type !== "session") return;
    const id = context.data.session.root(route.sessionID);
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
    context.data.on("session.execution.started", (e) => publish(e.data.sessionID, "working", true)),
    ...[
      "session.execution.succeeded",
      "session.execution.failed",
      "session.execution.interrupted",
    ].map((event) => context.data.on(event, (e) => publish(e.data.sessionID, "idle", true))),
  ];
  return () => stops.forEach((stop) => stop());
}

export function activityReporter(activity) {
  let child;
  let timer;
  let pending;
  let closed = false;
  const kill = () => {
    if (!child?.pid) return;
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      child.kill("SIGKILL");
    }
  };
  const drain = () => {
    if (closed || child || !pending) return;
    const state = pending;
    pending = undefined;
    child = spawn(activity, [state, "opencode"], { detached: true, stdio: "ignore" });
    timer = setTimeout(kill, 1500);
    const done = () => {
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
    if (
      !process.env.FORGE_SESSION_ID ||
      typeof context.options.activity !== "string" ||
      !context.options.activity
    )
      return;
    const reporter = activityReporter(context.options.activity);
    const dispose = observeActivity(context, reporter.report);
    return () => {
      dispose();
      reporter.close();
    };
  },
});
