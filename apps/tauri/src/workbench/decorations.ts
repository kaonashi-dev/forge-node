// Git decorations share the focused checkout's diff; external writes owe at
// most one refresh, held until the read finishes and its cooldown expires.
import { createComputed, createRoot, on } from "solid-js";
import { setLoading, setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import { fileChangesChannel } from "../runtime/bus";
import { loadDiff } from "./api";

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
  const workspace = workbenchStore.workspace;
  if (lastRead?.workspace !== workspace) lastRead = undefined;
  if (queued?.workspace !== workspace) queued = undefined;
  if (!queued || workbenchStore.loading.diff) return;
  pending = setTimeout(
    () => {
      pending = undefined;
      if (!queued || queued.workspace !== workbenchStore.workspace) return;
      if (!workbenchStore.loading.diff) read(queued.workspace);
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
  setWorkbenchStore("diffError", null);
  void loadDiff(workspace).catch((error: unknown) => {
    if (workbenchStore.workspace !== workspace || lastRead !== started) return;
    setWorkbenchStore("diffError", error instanceof Error ? error.message : String(error));
    setLoading("diff", false);
  });
}

/** Load decorations once; a failed read waits for a refresh or a new change. */
export function ensureDiff(): void {
  const workspace = workbenchStore.workspace;
  if (
    !workspace ||
    workbenchStore.diff ||
    workbenchStore.loading.diff ||
    workbenchStore.diffError
  ) {
    return;
  }
  read(workspace);
}

/** Explicit writes and gestures bypass the watch cooldown, but wait for an active read. */
export function refreshDiff(): void {
  const workspace = workbenchStore.workspace;
  if (!workspace) return;
  queued = { workspace, due: Date.now() + COALESCE_MS, explicit: true };
  schedule();
}

// Module lifetime matches the shared store and channel, independent of mounted panels.
createRoot(() => {
  createComputed(
    on(() => [workbenchStore.workspace, workbenchStore.loading.diff] as const, schedule),
  );
});

fileChangesChannel.subscribe(([workspace]) => {
  if (workspace !== workbenchStore.workspace || queued?.explicit) return;
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
