// Forge-owned OpenCode plugin: ring the session BEL when the agent asks the user.
// Loaded only for Forge-launched sessions (FORGE_SESSION_ID). See docs/agents.md.
import fs from "node:fs";

export const ForgeAttention = async () => {
  if (!process.env.FORGE_SESSION_ID) return {};
  const seen = new Set();
  const ring = () => {
    try {
      fs.writeFileSync("/dev/tty", "\x07");
    } catch {
      // Headless / no TTY: never fail the agent.
    }
  };
  return {
    event: ({ event }) => {
      if (
        event?.type !== "permission.asked" &&
        event?.type !== "question.asked"
      ) {
        return;
      }
      const id = event.properties?.id;
      if (id && seen.has(id)) return;
      if (id) seen.add(id);
      ring();
    },
  };
};
