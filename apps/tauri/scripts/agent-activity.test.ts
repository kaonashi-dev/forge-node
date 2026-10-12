import { expect, test } from "bun:test";

const source = await Bun.file(
  new URL("../../../crates/agents/src/forge-attention.js", import.meta.url),
).text();
const plugin = (await import(
  `data:text/javascript;base64,${Buffer.from(
    source.replace(
      'import { Plugin } from "@opencode/plugin/tui";',
      "const Plugin = { define: (definition) => definition };",
    ),
  ).toString("base64")}`
)) as {
  observeActivity: (context: unknown, report: (state: string) => void) => () => void;
};

test("OpenCode V2 reports root turns and family questions, never a child completion", () => {
  const handlers = new Map<string, (event: unknown) => void>();
  const states: string[] = [];
  const context = {
    ui: { router: { current: () => ({ type: "session", sessionID: "root" }) } },
    data: {
      session: { root: (id: string) => (id === "child" ? "root" : id) },
      on: (name: string, handler: (event: unknown) => void) => {
        handlers.set(name, handler);
        return () => handlers.delete(name);
      },
    },
  };
  const stop = plugin.observeActivity(context, (state) => states.push(state));
  const emit = (name: string, data: unknown) => handlers.get(name)?.({ data });
  expect(states).toEqual([]);
  emit("session.execution.started", { sessionID: "root" });
  emit("session.execution.started", { sessionID: "root" });
  emit("session.execution.succeeded", { sessionID: "child" });
  emit("permission.asked", { sessionID: "unrelated" });
  emit("permission.asked", { sessionID: "child" });
  emit("permission.replied", { sessionID: "child" });
  emit("form.created", { form: { sessionID: "root" } });
  emit("form.replied", { sessionID: "root" });
  emit("session.execution.succeeded", { sessionID: "root" });
  expect(states).toEqual(["working", "waiting", "working", "waiting", "working", "idle"]);
  stop();
  expect(handlers.size).toBe(0);
});
