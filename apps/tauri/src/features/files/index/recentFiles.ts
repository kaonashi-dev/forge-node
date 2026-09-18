// Which files were opened lately, per checkout, and what the palette shows
// before anything is typed.
//
// The tree comes from `git ls-files` and carries paths and nothing else — no
// mtime, no order but the repository's own. So "recently edited" is answered
// from two things the shell already knows: what was opened here, and what has
// uncommitted changes. Nothing in this file imports Solid or the daemon; the
// store wrapper at the bottom is the only part that does.

import { forgeStore } from "../../../state/forgeStore";
import { setAppState } from "../../settings/commands";
import { retargetPath } from "../../../shared/paths";

/** Where the per-checkout lists live in `app_state`. */
export const RECENT_FILES_KEY = "ui.files.recent";

/**
 * How many paths are remembered per checkout.
 *
 * Larger than what the palette shows, so that a file opened a while ago still
 * ranks once its name is typed — the memory is the candidate pool, the limit
 * below is only the opening screenful.
 */
export const RECENT_MEMORY = 50;

/** How many rows the file list opens on. */
export const RECENT_FILE_LIMIT = 30;

/** Checkout id to the paths opened in it, most recent first. */
export type RecentFiles = Record<string, string[]>;

/**
 * Read the stored lists, treating anything unexpected as "nothing remembered".
 *
 * A preference written by another build, or half-written, must degrade to an
 * empty palette rather than throwing inside a `createMemo` and taking the
 * whole palette down with it.
 */
export function parseRecent(raw: string | undefined): RecentFiles {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
  const out: RecentFiles = {};
  for (const [workspace, paths] of Object.entries(parsed as Record<string, unknown>)) {
    if (!Array.isArray(paths)) continue;
    const clean = paths.filter((path): path is string => typeof path === "string");
    if (clean.length > 0) out[workspace] = clean.slice(0, RECENT_MEMORY);
  }
  return out;
}

/** The lists with `path` moved to the front of `workspace`'s. */
export function withOpened(current: RecentFiles, workspace: string, path: string): RecentFiles {
  const previous = current[workspace] ?? [];
  // Moved rather than appended: opening a file again is what makes it recent,
  // and a list that kept the first visit would go stale while being written to.
  const next = [path, ...previous.filter((item) => item !== path)].slice(0, RECENT_MEMORY);
  return { ...current, [workspace]: next };
}

/**
 * The file rows shown before anything is typed.
 *
 * Recently opened first, then whatever has uncommitted changes — the second
 * half is what makes this useful in a checkout the person has just switched
 * to, where nothing has been opened yet but an agent has been writing.
 *
 * `known` is the tree, and a path missing from it is dropped rather than
 * offered: a remembered file can have been deleted, renamed, or belong to a
 * branch that has since been checked out over.
 */
export function initialFiles(
  recent: readonly string[],
  changed: readonly string[],
  known: ReadonlySet<string>,
  limit: number = RECENT_FILE_LIMIT,
): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const path of [...recent, ...changed]) {
    if (out.length >= limit) break;
    if (seen.has(path) || !known.has(path)) continue;
    seen.add(path);
    out.push(path);
  }
  return out;
}

/** The paths opened in `workspace`, most recent first. */
export function recentPaths(workspace: string | null): string[] {
  if (!workspace) return [];
  return parseRecent(forgeStore.app_state[RECENT_FILES_KEY])[workspace] ?? [];
}

/**
 * Remember that `path` was opened in `workspace`.
 *
 * Fire-and-forget: this rides on opening a file, and a preference that failed
 * to persist must not turn into an error where the person asked for a file.
 */
export function noteFileOpened(workspace: string | null, path: string): void {
  if (!workspace) return;
  const current = parseRecent(forgeStore.app_state[RECENT_FILES_KEY]);
  if (current[workspace]?.[0] === path) return;
  const next = withOpened(current, workspace, path);
  void setAppState(RECENT_FILES_KEY, JSON.stringify(next)).catch(() => undefined);
}

export function withMoved(
  current: RecentFiles,
  workspace: string,
  from: string,
  to: string,
): RecentFiles {
  const previous = current[workspace];
  if (!previous) return current;
  const next = [...new Set(previous.map((path) => retargetPath(path, from, to)))];
  if (next.length === previous.length && next.every((path, index) => path === previous[index]))
    return current;
  return { ...current, [workspace]: next };
}

export function retargetRecentFiles(workspace: string, from: string, to: string): void {
  const current = parseRecent(forgeStore.app_state[RECENT_FILES_KEY]);
  const next = withMoved(current, workspace, from, to);
  if (next === current) return;
  void setAppState(RECENT_FILES_KEY, JSON.stringify(next)).catch(() => undefined);
}
