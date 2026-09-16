// View interests share one connection-scoped watch; persistence stays in the daemon.
import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { fileChangesChannel } from "../runtime/bus";
import { runtimeStore } from "../store/runtimeStore";

type Interest = { workspace: string; directories: readonly string[] };
const interests = new Map<symbol, Interest>();
const [error, setError] = createSignal<string | null>(null);
export const fileWatchError = error;
let timer: ReturnType<typeof setTimeout> | undefined;
let previous: { workspace: string; key: string; directories: readonly string[] } | undefined;
// Newly watched folders may have changed while closed, even on a live connection.
let armed = false;
const resync = new Map<string, Set<string>>();
/* What the note says once the watch is in place: the budget warning, or
   nothing. Kept apart from a failure message so an ack clears the failure
   without also clearing the truncation it has no answer for. */
let standing: string | null = null;

function update(): void {
  clearTimeout(timer);
  timer = setTimeout(() => {
    timer = undefined;
    if (runtimeStore.connection.kind !== "connected") return;
    const current = [...interests.values()].at(-1);
    const workspace = current?.workspace ?? previous?.workspace;
    if (!workspace) return;
    const all = [
      ...new Set(
        [...interests.values()]
          .filter((item) => item.workspace === workspace)
          .flatMap((item) => [...item.directories]),
      ),
    ].sort();
    const directories = all.slice(0, 128);
    const key = JSON.stringify(directories);
    if (previous?.workspace === workspace && previous.key === key) return;
    const missed = resync.get(workspace) ?? new Set<string>();
    if (!armed || previous?.workspace !== workspace) missed.add("");
    else {
      for (const path of directories) {
        if (!previous.directories.includes(path)) missed.add(path);
      }
    }
    if (missed.has("") || missed.size > 128) {
      missed.clear();
      missed.add("");
    }
    if (missed.size > 0) resync.set(workspace, missed);
    previous = { workspace, key, directories };
    standing =
      all.length > 128
        ? "Live updates cover the first 128 open folders. Reload to refresh the full listing."
        : null;
    setError(standing);
    void invoke("send_workbench_command", {
      command: { type: "watch_files", workspace, directories },
    }).catch((error: unknown) => {
      previous = undefined;
      armed = false;
      setError(`Live updates unavailable: ${String(error)}`);
    });
  }, 100);
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
  update();
  return () => {
    interests.delete(id);
    unbind();
    update();
  };
}

/** Reconcile unwatched intervals only after the daemon has armed the directories. */
export function fileWatchReady(workspace: string): void {
  // An empty directory list is a release, acked like any other replacement: it
  // leaves nothing watching, so the next interest is an arm and not a swap.
  armed = (previous?.directories.length ?? 0) > 0;
  setError(standing);
  const paths = resync.get(workspace);
  resync.delete(workspace);
  if (!armed || !paths) return;
  for (const path of paths.has("") ? [""] : paths) {
    fileChangesChannel.publish([workspace, path]);
  }
}

export function reconnectFileWatches(): void {
  previous = undefined;
  armed = false;
  update();
}
export function failFileWatch(workspace: string, message: string): void {
  // Drop the memo so the next interest change retries instead of latching a
  // transient failure (a directory removed between listing and watch).
  if (previous?.workspace === workspace) previous = undefined;
  armed = false;
  setError(`Live updates unavailable: ${message}`);
}
