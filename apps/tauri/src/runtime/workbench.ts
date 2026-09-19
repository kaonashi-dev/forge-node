import { invoke } from "@tauri-apps/api/core";

/**
 * Workbench reads go to their own worker on the host, never to the thread that
 * carries terminal input: a `git diff` of a large checkout is seconds of
 * subprocess, and that thread also writes every keystroke to the PTY.
 */
export async function sendWorkbenchCommand(command: Record<string, unknown>): Promise<void> {
  try {
    await invoke("send_workbench_command", { command });
  } catch (error) {
    throw normalizeError(error);
  }
}

export function normalizeError(error: unknown): Error {
  if (error instanceof Error) return error;
  if (typeof error === "string") return new Error(error);
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return new Error(message);
  }
  try {
    return new Error(JSON.stringify(error) ?? String(error));
  } catch {
    return new Error(String(error));
  }
}
