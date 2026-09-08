// What the palette offers.
//
// Everything here is something the shell can already do; the palette is a
// second way in, not a second implementation. Providers that are not installed
// stay listed and dimmed, exactly as the launcher shows them, so the palette
// also answers "what could run here?".

import { ACTIONS, type ActionId } from "../actions/actions";
import { boundChord } from "../actions/bindings";
import { actionIsBound } from "../actions/dispatch";
import { describeChord } from "../actions/keys";
import { filter } from "./fuzzy";
import type { ShellSnapshot } from "../runtime/types";
import {
  sessionIsActive,
  sessionIsAgent,
  sessionStateLabel,
  sessionTitle,
  type Session,
  type Workspace,
} from "../runtime/types";

/**
 * Group headings. Named constants because `admits` matches on them: a heading
 * and its scope drifting apart is a silent bug.
 */
export const CREATE = "Create";
export const SESSIONS = "Sessions";
export const BRANCHES = "Branches";
export const FILES = "Files";
export const COMMANDS = "Commands";

export type PaletteGroup =
  | typeof CREATE
  | typeof SESSIONS
  | typeof BRANCHES
  | typeof FILES
  | typeof COMMANDS;

/**
 * Which half of the palette a shortcut opened it on.
 *
 * The palette answers two different questions — *where do I go* and *what do I
 * run* — and `cmd-k` asks both at once, which is right when you do not know
 * which one you have. When you do know, a list holding the other half is
 * noise: "main" is a branch, a session title and a substring of two commands.
 */
export type PaletteScope = "everything" | "places" | "commands" | "launch" | "files";

/**
 * The order groups appear in, and the order a filtered list is rebuilt in.
 *
 * The palette ranks *within* a group rather than across the whole list: the
 * headings are emitted as the group changes going down, so a global ranking
 * that interleaved two groups would print one heading over rows belonging to
 * both. Grouping also keeps the answer to "where do I go" — a handful of
 * sessions and checkouts — above the thousands of files underneath it.
 */
const GROUP_ORDER: PaletteGroup[] = [CREATE, SESSIONS, BRANCHES, FILES, COMMANDS];

/**
 * How many files a mixed list shows.
 *
 * `⌘⇧O` lists a whole repository's worth of paths; every one of them is a DOM
 * row, and past the first screenful nobody is reading them — they are typing.
 * The cap applies after ranking, so the best matches are the ones kept.
 */
const FILE_LIMIT = 200;

/**
 * Whether an entry under `group` belongs in this scope.
 *
 * `commands` is written as the *complement* of the others on purpose: a group
 * added later lands in it by default, which is a harmless miscategorisation,
 * where an explicit list would silently drop the new group out of every
 * narrowed palette.
 */
export function admits(scope: PaletteScope, group: string): boolean {
  switch (scope) {
    case "everything":
      return true;
    case "launch":
      return group === CREATE;
    case "files":
      return group === FILES;
    case "places":
      // Files too: "go to" is one question, and where a thing lives — a
      // session, a checkout, a path — is not something you want to have to
      // decide before you start typing (§16.4).
      return group === SESSIONS || group === BRANCHES || group === FILES;
    case "commands":
      return group !== SESSIONS && group !== BRANCHES && group !== CREATE && group !== FILES;
  }
}

export function scopeLabel(scope: PaletteScope): string | null {
  return {
    everything: null,
    places: "Go to",
    commands: "Command",
    launch: "New",
    files: "File",
  }[scope];
}

export function scopePlaceholder(scope: PaletteScope): string {
  return {
    everything: "Type a command…",
    places: "Go to a session, a branch or a file…",
    commands: "Run a command…",
    launch: "Start a terminal or an agent…",
    files: "Open a file in this workspace…",
  }[scope];
}

/** What choosing an entry asks the shell to do. */
export type PaletteChoice =
  | { kind: "action"; action: ActionId }
  | { kind: "new_shell" }
  | { kind: "new_agent"; provider: string; profile: string | null }
  | { kind: "focus_session"; session: string }
  | { kind: "focus_workspace"; workspace: string }
  | { kind: "open_file"; path: string };

export type PaletteEntry = {
  /** The searchable label. */
  label: string;
  group: PaletteGroup;
  /** Keyboard shortcut, when the entry has one. */
  hint: string | null;
  /** Trailing note — "not installed", a branch name. */
  note: string | null;
  /** Dimmed but still listed, so the palette answers "what could run here?". */
  enabled: boolean;
  /**
   * What the query is scored against, when that is more than the label.
   *
   * A file's label is its basename, but `src/pal` is how anyone who knows the
   * tree looks for it; a session's label is a terminal title, and the branch
   * beside it is half of what makes one of five `~/d/forge-node` rows the one
   * meant.
   */
  search?: string;
  choice: PaletteChoice;
};

/** Every entry the palette knows, in presentation order. */
export function paletteEntries(store: ShellSnapshot, activeSession: string | null): PaletteEntry[] {
  const entries: PaletteEntry[] = [];

  for (const launchable of store.launchables) {
    entries.push({
      label: launchable.kind === "shell" ? "New Terminal" : `New ${launchable.label}`,
      group: CREATE,
      hint:
        launchable.kind === "shell"
          ? hintFor("new_terminal")
          : launchable.profile
            ? null
            : hintFor("new_agent"),
      note: launchable.detail,
      enabled: launchable.enabled,
      choice:
        launchable.kind === "shell"
          ? { kind: "new_shell" }
          : {
              kind: "new_agent",
              provider: launchable.provider ?? "",
              profile: launchable.profile,
            },
    });
  }

  // A session already on screen is not somewhere to go.
  //
  // Running ones first: several sessions in one checkout share a title — five
  // rows reading `~/d/forge-node  main` is what the list looks like by lunch —
  // and the one still running is almost always the one meant. The sort is
  // stable, so within each half the store's order survives.
  const reachable = store.sessions.filter((session) => session.id !== activeSession);
  const sessions = [
    ...reachable.filter((session) => sessionIsActive(session.state)),
    ...reachable.filter((session) => !sessionIsActive(session.state)),
  ];
  for (const session of sessions) {
    const workspace = store.workspaces.find((item) => item.id === session.workspace_id);
    const note = sessionNote(session, workspace);
    entries.push({
      label: sessionTitle(session),
      group: SESSIONS,
      hint: null,
      note,
      enabled: sessionIsActive(session.state),
      // The branch is the half of the row that tells two identical titles
      // apart, so it has to be typeable.
      search: note ? `${sessionTitle(session)} ${note}` : sessionTitle(session),
      choice: { kind: "focus_session", session: session.id },
    });
  }

  // A worktree-first model makes a branch a workspace, so switching branches
  // and switching workspaces are the same navigation (§14, P4).
  for (const workspace of store.workspaces) {
    const project = store.projects.find((item) => item.id === workspace.project_id);
    const label = workspace.branch ?? workspace.display_name ?? workspace.path;
    entries.push({
      label,
      group: BRANCHES,
      hint: null,
      note: project?.name ?? null,
      enabled: true,
      search: `${label} ${project?.name ?? ""}`,
      choice: { kind: "focus_workspace", workspace: workspace.id },
    });
  }

  for (const action of ACTIONS) {
    if (!action.palette) continue;
    // The launcher above already offers these, with the providers spelled out.
    if (action.id === "new_terminal" || action.id === "new_agent") continue;
    entries.push({
      label: action.label,
      group: COMMANDS,
      hint: hintFor(action.id),
      note: action.detail,
      // Listed either way, but an action nothing has registered a handler for
      // does nothing when chosen — so it reads as unavailable rather than as a
      // command that silently fails.
      enabled: actionIsBound(action.id),
      choice: { kind: "action", action: action.id },
    });
  }

  return entries;
}

/** File palette entries from the cached tree of the active workspace. */
export function fileEntries(
  workspaceId: string | null,
  treePaths: string[] | null,
): PaletteEntry[] {
  if (!workspaceId || !treePaths) return [];
  return treePaths.map((path) => ({
    label: path.slice(path.lastIndexOf("/") + 1) || path,
    group: FILES,
    hint: null,
    note: path,
    enabled: true,
    // Scored on the whole path: `src/pal` is how anyone who knows the tree
    // reaches for `entries.ts`, and the basename alone cannot match it.
    search: path,
    choice: { kind: "open_file", path },
  }));
}

/**
 * The trailing note on a session row: where it is, and — when it is no longer
 * running — what became of it.
 *
 * A dimmed row with no explanation reads as broken; `main · exited 1` reads as
 * finished, which is the difference between hunting for the live one and
 * knowing you already found it.
 */
function sessionNote(session: Session, workspace: Workspace | undefined): string | null {
  const place = workspace?.branch ?? workspace?.display_name ?? null;
  if (sessionIsActive(session.state)) return place;
  const state = sessionStateLabel(session.state);
  return place ? `${place} · ${state}` : state;
}

/**
 * The list the palette shows: every entry the scope admits, ranked by `query`
 * inside its own group and rebuilt in group order.
 *
 * Ranking per group rather than across the whole list is what keeps the
 * headings honest — the list prints a heading where the group changes, so a
 * global sort that put a file between two sessions would file it under
 * "Sessions". It also keeps a repository's worth of paths from burying the
 * three sessions and two checkouts that `⌘⇧O` is usually reaching for.
 */
export function rank(entries: PaletteEntry[], query: string): PaletteEntry[] {
  const groups = [
    ...GROUP_ORDER,
    // A group added later still gets listed, at the end, rather than dropped.
    ...entries.map((entry) => entry.group).filter((group) => !GROUP_ORDER.includes(group)),
  ];
  const ranked: PaletteEntry[] = [];
  const seen = new Set<PaletteGroup>();
  for (const group of groups) {
    if (seen.has(group)) continue;
    seen.add(group);
    const matched = filter(
      entries.filter((entry) => entry.group === group),
      query,
    );
    ranked.push(...(group === FILES ? matched.slice(0, FILE_LIMIT) : matched));
  }
  return ranked;
}

/** Whether an agent session is what the label should read as. */
export function entryIsAgent(store: ShellSnapshot, entry: PaletteEntry): boolean {
  if (entry.choice.kind !== "focus_session") return entry.choice.kind === "new_agent";
  const id = entry.choice.session;
  const session = store.sessions.find((item) => item.id === id);
  return session ? sessionIsAgent(session) : false;
}

function hintFor(action: ActionId): string | null {
  const chord = boundChord(action);
  return chord ? describeChord(chord) : null;
}
