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
let previous: { workspace: string; key: string; watching: boolean } | undefined;
/* A watch is live on this connection and no events have been missed since it
   was armed. An interest change replaces the daemon-side watch without a gap,
   so only an arm — first watch, a new checkout, a reconnect, a retry after a
   failure, the first interest after the last one went away — leaves a window
   the views have to reconcile for. Held per workspace, because two arms can be
   in flight and each ack has to answer for its own. */
let armed = false;
const resync = new Set<string>();
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
    if (!armed || previous?.workspace !== workspace) resync.add(workspace);
    previous = { workspace, key, watching: directories.length > 0 };
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

/**
 * The daemon took the watch.
 *
 * The ack is not itself news about the checkout: the directory set is replaced
 * in place on every fold and every file opened outside the tree, and treating
 * each one as "everything changed" made an ordinary click re-list the checkout
 * and re-read the open file — the flicker. Only an arm, which had a window
 * where events could be lost, asks the views to reconcile.
 */
export function fileWatchReady(workspace: string): void {
  // An empty directory list is a release, acked like any other replacement: it
  // leaves nothing watching, so the next interest is an arm and not a swap.
  armed = previous?.watching ?? false;
  setError(standing);
  if (!armed || !resync.delete(workspace)) return;
  fileChangesChannel.publish([workspace, ""]);
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
