// View interests share a bounded connection-scoped watch with correlated acknowledgements.
import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { fileChangesChannel } from "../../../runtime/bus";
import { connectionStore } from "../../../state/connection";
import { directories as directoryState } from "../directories/directoryState";

type Interest = { workspace: string; directories: readonly string[] };
type WatchSet = Interest & { generation: number; key: string };
const interests = new Map<symbol, Interest>();
const [error, setError] = createSignal<string | null>(null);
export const fileWatchError = error;
export const WATCH_ACK_TIMEOUT_MS = 10_000;
const MAX_RETRIES = 3;
let generation = 0;
let timer: ReturnType<typeof setTimeout> | undefined;
let ackTimer: ReturnType<typeof setTimeout> | undefined;
let confirmed: WatchSet | undefined;
let pending: WatchSet | undefined;
let lastWorkspace: string | undefined;
let retries = 0;
let standing: string | null = null;
let force = false;
let interrupted = false;

function update(delay = 100): void {
  if (timer !== undefined) return;
  timer = setTimeout(() => {
    timer = undefined;
    if (connectionStore.connection.kind !== "connected") return;
    const workspace = [...interests.values()].at(-1)?.workspace ?? lastWorkspace;
    if (!workspace) return;
    lastWorkspace = workspace;
    const all = [
      ...new Set(
        [...interests.values()]
          .filter((item) => item.workspace === workspace)
          .flatMap((item) => [...item.directories]),
      ),
    ].sort();
    const enc = new TextEncoder();
    const valid = all.filter((path) => path.length <= 4096 && enc.encode(path).length <= 4096);
    const directories = valid.slice(0, 128);
    const key = JSON.stringify(directories);
    standing =
      directories.length !== all.length
        ? "Live updates cover at most 128 folders with paths up to 4096 bytes. Refresh to update uncovered folders."
        : null;
    setError(standing);
    if (
      !force &&
      ((pending?.workspace === workspace && pending.key === key) ||
        (!pending && confirmed?.workspace === workspace && confirmed.key === key))
    )
      return;
    force = false;
    clearTimeout(ackTimer);
    interrupted ||= pending !== undefined;
    const request = { workspace, directories, key, generation: ++generation };
    pending = request;
    setError(standing);
    ackTimer = setTimeout(
      () => failFileWatch(workspace, "Watch acknowledgement timed out.", request.generation),
      WATCH_ACK_TIMEOUT_MS,
    );
    void invoke("send_workbench_command", {
      command: {
        type: "watch_files",
        workspace,
        directories,
        generation: request.generation,
      },
    }).catch((failure: unknown) => failFileWatch(workspace, String(failure), request.generation));
  }, delay);
}

export function watchFiles(
  workspace: string,
  directories: readonly string[],
  changed: (path: string) => void,
): () => void {
  const id = Symbol();
  interests.set(id, { workspace, directories });
  const unbind = fileChangesChannel.subscribe(([source, path]) => {
    if (source === workspace) changed(path);
  });
  retries = 0;
  update();
  return () => {
    interests.delete(id);
    unbind();
    retries = 0;
    update();
  };
}

export function fileWatchReady(workspace: string, acknowledgedGeneration: number): void {
  if (!pending || pending.workspace !== workspace || pending.generation !== acknowledgedGeneration)
    return;
  clearTimeout(ackTimer);
  const ready = pending;
  pending = undefined;
  const previous = confirmed;
  confirmed = ready;
  retries = 0;
  setError(standing);
  if (ready.directories.length === 0) return;
  const paths =
    interrupted ||
    !previous ||
    previous.workspace !== workspace ||
    previous.directories.length === 0
      ? [""]
      : ready.directories.filter((path) => !previous.directories.includes(path));
  interrupted = false;
  for (const path of paths) {
    directoryState.invalidate(workspace, path ? [path] : undefined);
    fileChangesChannel.publish([workspace, path]);
  }
}

export function disconnectFileWatches(): void {
  clearTimeout(timer);
  timer = undefined;
  clearTimeout(ackTimer);
  confirmed = undefined;
  pending = undefined;
  interrupted = false;
  retries = 0;
  generation++;
}

export function reconnectFileWatches(): void {
  disconnectFileWatches();
  update();
}

/** A directory recreated under the same name needs a new native watch. */
export function rearmFileWatches(workspace: string, path: string): void {
  if (
    path &&
    ![...interests.values()].some(
      (interest) =>
        interest.workspace === workspace &&
        interest.directories.some(
          (directory) => directory === path || directory.startsWith(`${path}/`),
        ),
    )
  )
    return;
  if (lastWorkspace !== workspace) return;
  confirmed = undefined;
  force = true;
  retries = 0;
  update();
}

export function failFileWatch(workspace: string, message: string, failedGeneration: number): void {
  if (!pending || pending.workspace !== workspace || pending.generation !== failedGeneration)
    return;
  clearTimeout(ackTimer);
  pending = undefined;
  confirmed = undefined;
  setError(`Live updates unavailable: ${message}`);
  if (retries < MAX_RETRIES) update(500 * 2 ** retries++);
}
