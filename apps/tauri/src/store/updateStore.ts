import { createSignal } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { checkForUpdate, installUpdate } from "../runtime/api";
import type { UpdateState } from "../runtime/types";

/**
 * The last thing the host said about updates.
 *
 * The host owns the schedule, the network and the protocol classification
 * (`src-tauri/src/updates.rs`); this is a mailbox for its one event, so the
 * status bar can render without asking anything.
 */
const [state, setState] = createSignal<UpdateState | null>(null);

/**
 * Whether the current state answers a gesture rather than the six-hour clock.
 *
 * It decides whether "up to date" and "failed" are worth showing at all: a
 * background check that finds nothing must stay invisible, and the same state
 * from the menu item must not.
 */
const [userAsked, setUserAsked] = createSignal(false);

export const updateState = state;
export const updateWasRequested = userAsked;

export function listenForUpdates(): Promise<() => void> {
  return listen<UpdateState>("shell:update", (event) => {
    setState(event.payload);
    // The host emits `available` from both paths; a background find is news
    // the user did not ask for and must not resurrect an old gesture's
    // "up to date".
    if (event.payload.state === "available") setUserAsked(false);
  });
}

/** The `Check for Updates…` menu item. Reports either way. */
export async function requestUpdateCheck(): Promise<void> {
  setUserAsked(true);
  await checkForUpdate().catch(() => undefined);
}

/**
 * Apply the pending update. Never resolves on success — the process is
 * replaced — so callers must not sequence anything after it.
 */
export async function applyUpdate(): Promise<void> {
  setUserAsked(true);
  await installUpdate().catch(() => undefined);
}
