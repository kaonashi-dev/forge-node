import type { Launchable, ShellSnapshot, Workspace } from "../runtime/types";

/** `git-service::worktree::MAX_SLUG_LEN`. */
const MAX_SLUG_LEN = 64;
/** `git-service::worktree::FALLBACK_SLUG`. */
const FALLBACK_SLUG = "worktree";

/**
 * The directory name the daemon would derive from a branch.
 *
 * A mirror of `git_service::worktree::slugify`, and only ever used as the
 * *placeholder* under the name field: the daemon still derives the real one,
 * and `unique_slug` may add a `-2` this side cannot know about. Showing it is
 * what makes the field optional — you can see what you would get before
 * deciding you want something else.
 */
export function slugFromBranch(branch: string): string {
  let out = "";
  let previousDash = false;
  for (const char of branch) {
    const mapped = /[A-Za-z0-9._-]/.test(char) && char !== "/" ? char : "-";
    if (mapped === "-") {
      if (previousDash) continue;
      previousDash = true;
    } else {
      previousDash = false;
    }
    out += mapped;
    if (out.length >= MAX_SLUG_LEN) break;
  }
  return out === "" || out === "." || out === ".." ? FALLBACK_SLUG : out;
}

/** What to start in the new checkout once it exists. */
export type LaunchChoice =
  | { kind: "none" }
  | { kind: "shell" }
  | { kind: "agent"; provider: string; profile: string | null };

/** The value a `Select` option carries, and the choice it maps back to. */
export const NOTHING = "none";
const SHELL = "shell";

/**
 * The "Open with" list: nothing, a terminal, then whatever can launch here.
 *
 * Built from `launchables` — the same list the `+` menu and the palette read —
 * so an agent that is not installed is listed and disabled here for the same
 * reason it is there: the menu also answers "what could run in this checkout?".
 */
export function launchOptions(
  launchables: Launchable[],
): { value: string; label: string; disabled: boolean }[] {
  return [
    { value: NOTHING, label: "Nothing — just create it", disabled: false },
    ...launchables.map((launchable) => ({
      value: launchable.key,
      label: launchable.kind === "shell" ? "New Terminal" : `New ${launchable.label}`,
      disabled: !launchable.enabled,
    })),
  ];
}

/** Turn a selected option back into what to launch. */
export function parseLaunch(value: string, launchables: Launchable[]): LaunchChoice {
  if (value === NOTHING) return { kind: "none" };
  const launchable = launchables.find((item) => item.key === value);
  if (!launchable || !launchable.enabled) return { kind: "none" };
  if (launchable.kind === "shell") return { kind: "shell" };
  return { kind: "agent", provider: launchable.provider ?? "", profile: launchable.profile };
}

/**
 * What the dialog should open on: a terminal, when there is one to open.
 *
 * "Nothing" is the wrong default for a dialog whose whole point is starting
 * work somewhere new — creating the checkout and then hunting for it in the
 * rail is the step this is meant to remove.
 */
export function defaultLaunch(launchables: Launchable[]): string {
  return launchables.find((item) => item.kind === "shell" && item.enabled)?.key ?? SHELL;
}

/**
 * The checkout the daemon just made, once its broadcast has landed.
 *
 * `CreateWorktree` acks and then broadcasts like every other mutation, so the
 * new workspace has no id at the call site; it is recognised here by the
 * branch it was created for, which is unique per project because git will not
 * check the same branch out twice.
 */
export function createdWorkspace(
  store: ShellSnapshot,
  project: string,
  branch: string,
): Workspace | null {
  return (
    store.workspaces.find(
      (workspace) => workspace.project_id === project && workspace.branch === branch,
    ) ?? null
  );
}
