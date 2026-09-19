import { expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { sendTargetedPaste } from "./commands";
import { setConnectionStore } from "../../state/connection";

it("captures both destination identities and the connection generation before IPC yields", async () => {
  setConnectionStore({
    activeSession: "other",
    activeTerminal: "other-terminal",
    connectionGeneration: 7,
  });
  const pending = sendTargetedPaste("drop-session", "drop-terminal", "'/checkout/a b'");
  setConnectionStore({
    activeSession: "new",
    activeTerminal: "new-terminal",
    connectionGeneration: 8,
  });
  await pending;
  expect(invoke).toHaveBeenCalledWith("send_runtime_command", {
    command: {
      type: "paste_target",
      session_id: "drop-session",
      terminal_id: "drop-terminal",
      text: "'/checkout/a b'",
      connection_generation: 7,
    },
  });
});
