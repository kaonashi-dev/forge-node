// Git decorations share the focused checkout's diff; external writes owe at
// most one refresh, held until the read finishes and its cooldown expires.
import { createComputed, createRoot, on } from "solid-js";
import { fileChangesChannel } from "../../runtime/bus";
import { loadDiff } from "./commands";
import { gitStore, setGitStore } from "./state";
import { loading, setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";

const COALESCE_MS = 150;
// Git subprocesses cost seconds on large checkouts; explicit gestures bypass this floor.
const WATCH_FLOOR_MS = 10_000;

type Refresh = { workspace: string; due: number; explicit: boolean };
let queued: Refresh | undefined;
let pending: ReturnType<typeof setTimeout> | undefined;
let lastRead: { workspace: string; at: number } | undefined;

function schedule(): void {
  clearTimeout(pending);
  pending = undefined;
  const workspace = activeWorkspace();
  if (lastRead?.workspace !== workspace) lastRead = undefined;
  if (queued?.workspace !== workspace) queued = undefined;
  if (!queued || loading.diff) return;
  pending = setTimeout(
    () => {
      pending = undefined;
      if (!queued || queued.workspace !== activeWorkspace()) return;
      if (!loading.diff) read(queued.workspace);
    },
    Math.max(0, queued.due - Date.now()),
  );
}

function read(workspace: string): void {
  queued = undefined;
  clearTimeout(pending);
  pending = undefined;
  const started = { workspace, at: Date.now() };
  lastRead = started;
  setLoading("diff", true);
  setGitStore("diffError", null);
  void loadDiff(workspace).catch((error: unknown) => {
    if (activeWorkspace() !== workspace || lastRead !== started) return;
    setGitStore("diffError", error instanceof Error ? error.message : String(error));
    setLoading("diff", false);
  });
}

/** Load decorations once; a failed read waits for a refresh or a new change. */
export function ensureDiff(): void {
  const workspace = activeWorkspace();
  if (!workspace || gitStore.diff || loading.diff || gitStore.diffError) {
    return;
  }
  read(workspace);
}

/** Explicit writes and gestures bypass the watch cooldown, but wait for an active read. */
export function refreshDiff(): void {
  const workspace = activeWorkspace();
  if (!workspace) return;
  queued = { workspace, due: Date.now() + COALESCE_MS, explicit: true };
  schedule();
}

// App lifetime matches the shared store and channel, independent of mounted panels.
export function startDecorationEffects(): () => void {
  const disposeRoot = createRoot((dispose) => {
    createComputed(on(() => [activeWorkspace(), loading.diff] as const, schedule));
    return dispose;
  });
  const unsubscribe = fileChangesChannel.subscribe(([workspace]) => {
    if (workspace !== activeWorkspace() || queued?.explicit) return;
    const now = Date.now();
    queued ??= {
      workspace,
      due: Math.max(
        now + COALESCE_MS,
        lastRead?.workspace === workspace ? lastRead.at + WATCH_FLOOR_MS : now,
      ),
      explicit: false,
    };
    schedule();
  });
  return () => {
    unsubscribe();
    disposeRoot();
  };
}
