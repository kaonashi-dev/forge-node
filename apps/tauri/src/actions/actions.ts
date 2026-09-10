// The shell's actions and their default bindings — port of
// `theme tokens::actions` (§16.6).
//
// Every chrome shortcut is a named action bound in a context, not a match on a
// raw keystroke. That distinction is load-bearing for a terminal app: the
// dispatcher runs in the capture phase and stops propagation on a match, so a
// bound shortcut cannot leak into the PTY — and, unlike a hand-rolled matcher,
// the rule holds on Linux, where the shortcut modifier is `ctrl` and `ctrl-b`
// would otherwise be perfectly good terminal input.
//
// Contexts nest: a binding declared on `App` fires while focus is anywhere in
// the window, and one declared on `Terminal` only fires with the grid focused.

import { CLIPBOARD_MOD, MOD, PASTE_CHORD, parseChord, type Chord } from "./keys";

/** The context on the window root: everything the shell can do from anywhere. */
export const APP = "App";
/** The project rail. */
export const SIDEBAR = "Sidebar";
/** The terminal grid. */
export const TERMINAL = "Terminal";
/** The in-app file editor, while its pane holds the keyboard (ADR-012). */
export const EDITOR = "Editor";
/**
 * The right panel's file tree, while it holds the keyboard.
 *
 * Its bindings are bare letters (`j`, `k`, `h`, `l`), which is only safe
 * because a context nests: they exist while focus is inside the tree and
 * nowhere else. Bound app-wide they would take four letters away from every
 * terminal on screen.
 */
export const FILES = "Files";
/** The command palette overlay, while it is open. */
export const COMMAND_PALETTE = "CommandPalette";

export type ContextId =
  | typeof APP
  | typeof SIDEBAR
  | typeof TERMINAL
  | typeof EDITOR
  | typeof FILES
  | typeof COMMAND_PALETTE;

/**
 * Innermost first. A binding in a nested context outbids the same chord on
 * `App`, which is how `cmd-c` copies the terminal rather than doing whatever
 * the shell would have done with it.
 */
/**
 * Innermost first. A binding in an earlier context wins.
 *
 * `EDITOR` sits above `FILES` deliberately (`plan-ui-ux.md` §8.1): CodeMirror's
 * own keymap runs *inside* the editor context, and a chord the editor claims
 * must not be answered by a tree that merely happens to be on screen beside
 * it. `FILES` is only entered while the tree actually holds focus, so in
 * practice the two are never active at once — this is the belt to that brace.
 */
export const CONTEXT_ORDER: ContextId[] = [COMMAND_PALETTE, EDITOR, FILES, TERMINAL, SIDEBAR, APP];

export type ActionId =
  | "about"
  | "check_for_update"
  | "open_command_palette"
  | "go_to"
  | "find_command"
  | "new_terminal"
  | "new_agent"
  | "new_worktree"
  | "new_feature"
  | "open_settings"
  | "close_session"
  | "next_session"
  | "previous_session"
  | "switch_tab_next"
  | "switch_tab_previous"
  | "focus_session"
  | "toggle_sidebar"
  | "toggle_right_panel"
  | "toggle_pull_requests"
  | "toggle_files"
  | "toggle_features"
  | "toggle_lieutenant"
  | "add_project"
  | "open_file_palette"
  | "save_file"
  | "go_to_definition"
  | "file_tree_next"
  | "file_tree_previous"
  | "file_tree_collapse"
  | "file_tree_expand"
  | "file_tree_open"
  | "file_tree_first"
  | "file_tree_last"
  | "file_tree_rename"
  | "file_tree_delete"
  | "file_tree_refresh"
  | "copy_terminal"
  | "paste_terminal"
  | "scroll_up"
  | "scroll_down"
  /* U3 — the chords the shell had a command for but no key. */
  | "terminal_zoom_in"
  | "terminal_zoom_out"
  | "terminal_zoom_reset"
  | "reopen_closed_tab"
  | "toggle_git"
  | "toggle_history"
  /* §16.7-16.8 — the three things you can do to the session on screen. */
  | "session_handoff"
  | "toggle_session_changes"
  | "review_checkout"
  | "close_other_views"
  | "close_views_to_right"
  | "previous_code_view"
  | "next_code_view"
  /* U4 — the project rail, which declared `role="tree"` and bound nothing. */
  | "sidebar_next"
  | "sidebar_previous"
  | "sidebar_collapse"
  | "sidebar_expand"
  | "sidebar_first"
  | "sidebar_last"
  | "sidebar_open"
  | "sidebar_new_terminal"
  /* U8 — the file tree's own filter. */
  | "file_tree_filter";

export type Action = {
  id: ActionId;
  /** What the palette calls it. */
  label: string;
  /** One line of what it does, for the palette's second row. */
  detail: string;
  /** Whether the command palette lists it. */
  palette: boolean;
};

/** Every action, in the order the palette lists them when nothing is typed. */
export const ACTIONS: Action[] = [
  {
    id: "new_terminal",
    label: "New Terminal",
    detail: "Shell session in the active workspace",
    palette: true,
  },
  {
    id: "new_agent",
    label: "New Agent",
    detail: "Launch an agent CLI in the active workspace",
    palette: true,
  },
  {
    id: "new_worktree",
    label: "New Worktree",
    detail: "Start a workspace on a new or existing branch",
    palette: true,
  },
  {
    id: "close_session",
    label: "Close",
    detail: "Close the open file or view, else the active session",
    palette: true,
  },
  {
    id: "next_session",
    label: "Next Session",
    detail: "Select the next tab, wrapping at the end",
    palette: true,
  },
  {
    id: "previous_session",
    label: "Previous Session",
    detail: "Select the previous tab",
    palette: true,
  },
  /* Not in the palette: the gesture *is* the hold, and a click on a menu entry
     releases nothing to commit. */
  {
    id: "switch_tab_next",
    label: "Switch Tab",
    detail: "Walk the recent-session ring",
    palette: false,
  },
  {
    id: "switch_tab_previous",
    label: "Switch Tab (Backward)",
    detail: "Walk the recent-session ring backward",
    palette: false,
  },
  {
    id: "toggle_sidebar",
    label: "Toggle Sidebar",
    detail: "Show or hide the project rail",
    palette: true,
  },
  {
    id: "toggle_right_panel",
    label: "Toggle Right Panel",
    detail: "Show or hide the inspector",
    palette: true,
  },
  {
    id: "toggle_pull_requests",
    label: "Pull Requests",
    detail: "Show the inspector's pull-request tab",
    palette: true,
  },
  {
    id: "toggle_files",
    label: "Files",
    detail: "Show the file browser in the inspector",
    palette: true,
  },
  {
    id: "toggle_features",
    label: "Features",
    detail: "Show the harness features in the inspector",
    palette: true,
  },
  { id: "toggle_lieutenant", label: "Lieutenant", detail: "Ask about the harness", palette: true },
  {
    id: "new_feature",
    label: "New Feature…",
    detail: "Draft a spec and register it with the harness",
    palette: true,
  },
  {
    id: "add_project",
    label: "Add Project…",
    detail: "Register a directory as a project",
    palette: true,
  },
  {
    id: "open_settings",
    label: "Settings",
    detail: "Show or hide the settings screen",
    palette: true,
  },
  { id: "copy_terminal", label: "Copy", detail: "Copy the terminal selection", palette: false },
  { id: "paste_terminal", label: "Paste", detail: "Paste into the terminal", palette: false },
  { id: "scroll_up", label: "Scroll Up", detail: "One page into the scrollback", palette: false },
  {
    id: "scroll_down",
    label: "Scroll Down",
    detail: "One page towards the live output",
    palette: false,
  },
  {
    id: "open_command_palette",
    label: "Command Palette",
    detail: "Everything the palette knows",
    palette: false,
  },
  { id: "go_to", label: "Go To…", detail: "Sessions, checkouts and files", palette: true },
  {
    id: "find_command",
    label: "Find Command…",
    detail: "Everything that does something",
    palette: true,
  },
  {
    id: "open_file_palette",
    label: "Open File…",
    detail: "Find a file in the active workspace",
    palette: true,
  },
  {
    id: "save_file",
    label: "Save File",
    detail: "Write the open file through the daemon",
    palette: false,
  },
  {
    id: "go_to_definition",
    label: "Go To Definition",
    detail: "Where the name under the caret is declared",
    palette: false,
  },
  /* U3. Everything below had a chord and no palette row, or the reverse. */
  {
    id: "terminal_zoom_in",
    label: "Terminal: Zoom In",
    detail: "One step larger, for this window",
    palette: true,
  },
  {
    id: "terminal_zoom_out",
    label: "Terminal: Zoom Out",
    detail: "One step smaller, for this window",
    palette: true,
  },
  {
    id: "terminal_zoom_reset",
    label: "Terminal: Actual Size",
    detail: "Back to the theme's own font size",
    palette: true,
  },
  {
    id: "reopen_closed_tab",
    label: "Reopen Closed Tab",
    detail: "Bring back the last view that was closed",
    palette: true,
  },
  { id: "toggle_git", label: "Git", detail: "Show the inspector's git tab", palette: true },
  {
    id: "session_handoff",
    label: "Continue in a New Session…",
    detail: "Carry this session's context into a fresh agent",
    palette: true,
  },
  {
    id: "toggle_session_changes",
    label: "Session Changes",
    detail: "Show what this session changed, beside its terminal",
    palette: true,
  },
  {
    id: "review_checkout",
    label: "Review This Checkout",
    detail: "Everything its sessions changed, with a summary",
    palette: true,
  },
  {
    id: "toggle_history",
    label: "History",
    detail: "Show the inspector's history tab",
    palette: true,
  },
  {
    id: "close_other_views",
    label: "Close Other Files",
    detail: "Close every open file but this one",
    palette: true,
  },
  {
    id: "close_views_to_right",
    label: "Close Files to the Right",
    detail: "Close everything after this one in the strip",
    palette: true,
  },
  {
    id: "previous_code_view",
    label: "Previous File",
    detail: "The file before this one in the strip",
    palette: true,
  },
  {
    id: "next_code_view",
    label: "Next File",
    detail: "The file after this one in the strip",
    palette: true,
  },
  /* U4. The rail's own keyboard. None of these belong in the palette: they
     move a selection that only exists while the rail has focus. */
  {
    id: "sidebar_next",
    label: "Sidebar: Next",
    detail: "Move the rail selection one row down",
    palette: false,
  },
  {
    id: "sidebar_previous",
    label: "Sidebar: Previous",
    detail: "Move the rail selection one row up",
    palette: false,
  },
  {
    id: "sidebar_collapse",
    label: "Sidebar: Collapse",
    detail: "Fold the row, or move to its parent",
    palette: false,
  },
  {
    id: "sidebar_expand",
    label: "Sidebar: Expand",
    detail: "Unfold the row, or move to its first child",
    palette: false,
  },
  {
    id: "sidebar_first",
    label: "Sidebar: First",
    detail: "Select the first row in the rail",
    palette: false,
  },
  {
    id: "sidebar_last",
    label: "Sidebar: Last",
    detail: "Select the last row in the rail",
    palette: false,
  },
  {
    id: "sidebar_open",
    label: "Sidebar: Open",
    detail: "Activate the selected row",
    palette: false,
  },
  {
    id: "sidebar_new_terminal",
    label: "Sidebar: New Terminal Here",
    detail: "Start a shell in the selected checkout",
    palette: false,
  },
  {
    id: "file_tree_filter",
    label: "File Tree: Filter",
    detail: "Focus the filter box above the tree",
    palette: false,
  },
  {
    id: "file_tree_first",
    label: "File Tree: First",
    detail: "Select the first row in the tree",
    palette: false,
  },
  {
    id: "file_tree_last",
    label: "File Tree: Last",
    detail: "Select the last row in the tree",
    palette: false,
  },
  {
    id: "file_tree_rename",
    label: "File Tree: Rename…",
    detail: "Rename the selected path through the daemon",
    palette: false,
  },
  {
    id: "file_tree_delete",
    label: "File Tree: Delete…",
    detail: "Delete the selected path, after asking",
    palette: false,
  },
  {
    id: "file_tree_refresh",
    label: "File Tree: Refresh",
    detail: "Re-read the checkout, so paths an agent added or removed appear",
    palette: true,
  },
  {
    id: "file_tree_next",
    label: "File Tree: Next",
    detail: "Move the selection one row down",
    palette: false,
  },
  {
    id: "file_tree_previous",
    label: "File Tree: Previous",
    detail: "Move the selection one row up",
    palette: false,
  },
  {
    id: "file_tree_collapse",
    label: "File Tree: Collapse",
    detail: "Fold, or jump to the parent",
    palette: false,
  },
  {
    id: "file_tree_expand",
    label: "File Tree: Expand",
    detail: "Unfold, or step into the first child",
    palette: false,
  },
  {
    id: "file_tree_open",
    label: "File Tree: Open",
    detail: "Open the file, or fold the directory",
    palette: false,
  },
  { id: "focus_session", label: "Focus Session", detail: "Focus a tab by number", palette: false },
  {
    id: "about",
    label: "About Forge Node",
    detail: "Version and daemon information",
    palette: true,
  },
  {
    id: "check_for_update",
    label: "Check for Updates",
    detail: "Ask whether a newer release exists",
    palette: true,
  },
];

export type Binding = {
  chord: Chord;
  action: ActionId;
  context: ContextId;
  /** `focus_session` carries the tab number the chord names. */
  argument?: number;
};

/**
 * The tabs the number chords can reach, counted the way the user counts them.
 *
 * Starts at 2 because `cmd-1` was spent on the project rail: the rail is the
 * only chord that gives the whole window back, and it is worth more than a
 * direct route to a tab that `ctrl-tab` already reaches in one press from
 * either neighbour. The numbering stays literal — `cmd-2` is tab 2.
 */
export const FOCUSABLE_SESSIONS = [2, 3, 4, 5, 6, 7, 8, 9];

/** The default bindings, built without touching the DOM so tests can read them. */
export function defaultBindings(): Binding[] {
  const bind = (
    spec: string,
    action: ActionId,
    context: ContextId,
    argument?: number,
  ): Binding => ({
    chord: parseChord(spec),
    action,
    context,
    argument,
  });

  const bindings: Binding[] = [
    bind(`${MOD}-k`, "open_command_palette", APP),
    // The two narrowed palettes take JetBrains' chords for the same two
    // questions: `shift-o` is "go to the thing I named", `shift-p` is "run the
    // thing I named". Both carry `shift` on purpose — on Linux `MOD` is
    // `ctrl`, and `ctrl-o`/`ctrl-p` are readline's own keys.
    bind(`${MOD}-shift-o`, "go_to", APP),
    bind(`${MOD}-shift-p`, "find_command", APP),
    bind(`${MOD}-t`, "new_terminal", APP),
    bind(`${MOD}-shift-a`, "new_agent", APP),
    bind(`${MOD}-shift-n`, "new_worktree", APP),
    bind(`${MOD}-,`, "open_settings", APP),
    bind(`${MOD}-w`, "close_session", APP),
    // Two chords for one rail, deliberately: `${MOD}-b` is what every editor
    // has taught, and `${MOD}-1` is where the hand already is when it reaches
    // for the tab numbers.
    bind(`${MOD}-b`, "toggle_sidebar", APP),
    bind(`${MOD}-1`, "toggle_sidebar", APP),
    bind(`${MOD}-j`, "toggle_right_panel", APP),
    bind(`${MOD}-shift-r`, "toggle_pull_requests", APP),
    bind(`${MOD}-shift-f`, "toggle_files", APP),
    // `shift-h` for the harness: `${MOD}-h` alone is macOS's hide-application,
    // and on Linux `ctrl-h` is a terminal's backspace.
    bind(`${MOD}-shift-h`, "toggle_features", APP),
    bind(`${MOD}-p`, "open_file_palette", APP),
    // Scoped to the editor, not to the app: on Linux `MOD` is `ctrl`, and
    // `ctrl-s` is a terminal's own key.
    bind(`${MOD}-s`, "save_file", EDITOR),
    /*
     * The same chord the rail has on `App`, claimed back inside the editor.
     * `EDITOR` outranks `APP` in `CONTEXT_ORDER`, so this only shadows the
     * rail while the editor holds the keyboard — and the rail keeps `MOD-1`,
     * which is the reason it can afford to lend the letter out at all.
     */
    bind(`${MOD}-b`, "go_to_definition", EDITOR),
    // Both the arrows and the `hjkl` block, in the tree's own context — the
    // one surface where the two audiences overlap completely.
    bind("down", "file_tree_next", FILES),
    bind("j", "file_tree_next", FILES),
    bind("up", "file_tree_previous", FILES),
    bind("k", "file_tree_previous", FILES),
    bind("left", "file_tree_collapse", FILES),
    bind("h", "file_tree_collapse", FILES),
    bind("right", "file_tree_expand", FILES),
    bind("l", "file_tree_expand", FILES),
    bind("enter", "file_tree_open", FILES),
    // Two gestures, deliberately different. The bracket chords are discrete
    // presses and walk the strip in its drag order; `ctrl-tab` is held, walks
    // the focus ring, and shows the switcher if the hold lasts. `ctrl` is
    // hardcoded because on macOS `cmd-tab` is the OS app switcher.
    bind(`${MOD}-shift-]`, "next_session", APP),
    bind(`${MOD}-shift-[`, "previous_session", APP),
    bind("ctrl-tab", "switch_tab_next", APP),
    bind("ctrl-shift-tab", "switch_tab_previous", APP),
    // Copy stays ours on every platform: the selection is drawn on a canvas,
    // so the WebView's own copy would take the hidden textarea's selection,
    // which is empty. Paste is the opposite — see `PASTE_CHORD`.
    bind(`${CLIPBOARD_MOD}-c`, "copy_terminal", TERMINAL),
    ...(PASTE_CHORD === null ? [] : [bind(PASTE_CHORD, "paste_terminal", TERMINAL)]),
    // A page rather than a line: a keyboard user scrolling a terminal is
    // looking for something that scrolled off, not nudging the view.
    bind(`${MOD}-up`, "scroll_up", TERMINAL),
    bind(`${MOD}-down`, "scroll_down", TERMINAL),
    /*
     * U3. Each of these had a command and no chord, which meant it existed
     * only for whoever knew to open the palette and type its name.
     *
     * `${MOD}-f` is deliberately *not* claimed app-wide: inside the editor it
     * is CodeMirror's find, and the editor's own keymap is the right place for
     * it (§8.1). Find-in-terminal, which §4.1 U3 also asks for, is not here
     * yet — it needs a search over the scrollback and a highlight in the cell
     * renderer, and a chord bound to nothing is the thing U3 exists to remove.
     */
    bind(`${MOD}-=`, "terminal_zoom_in", TERMINAL),
    bind(`${MOD}--`, "terminal_zoom_out", TERMINAL),
    bind(`${MOD}-0`, "terminal_zoom_reset", TERMINAL),
    bind(`${MOD}-shift-t`, "reopen_closed_tab", APP),
    bind(`${MOD}-shift-g`, "toggle_git", APP),
    bind(`${MOD}-shift-e`, "toggle_history", APP),
    bind(`${MOD}-shift-w`, "close_other_views", APP),
    bind(`${MOD}-alt-w`, "close_views_to_right", APP),
    bind(`${MOD}-alt-left`, "previous_code_view", APP),
    bind(`${MOD}-alt-right`, "next_code_view", APP),
    /*
     * U4. The rail declared `role="tree"` and bound nothing, so it was
     * reachable by Tab and operable by nothing. Bare letters, like the file
     * tree's, and safe for the same reason: the context is only entered while
     * the rail actually holds the keyboard.
     */
    bind("down", "sidebar_next", SIDEBAR),
    bind("j", "sidebar_next", SIDEBAR),
    bind("up", "sidebar_previous", SIDEBAR),
    bind("k", "sidebar_previous", SIDEBAR),
    bind("left", "sidebar_collapse", SIDEBAR),
    bind("h", "sidebar_collapse", SIDEBAR),
    bind("right", "sidebar_expand", SIDEBAR),
    bind("l", "sidebar_expand", SIDEBAR),
    bind("home", "sidebar_first", SIDEBAR),
    bind("end", "sidebar_last", SIDEBAR),
    bind("enter", "sidebar_open", SIDEBAR),
    bind(`${MOD}-enter`, "sidebar_new_terminal", SIDEBAR),
    /* U8. `${MOD}-f` again, in the tree's own context — "find in this panel"
       is what the chord means everywhere, and the context decides which. */
    bind(`${MOD}-f`, "file_tree_filter", FILES),
    bind("home", "file_tree_first", FILES),
    bind("end", "file_tree_last", FILES),
    /* A11. `F2` and `⌫` on the selected row, the way every file browser has
       taught. Both ask before they act. */
    bind(`${MOD}-r`, "file_tree_refresh", FILES),
    bind("f2", "file_tree_rename", FILES),
    bind("backspace", "file_tree_delete", FILES),
  ];

  for (const index of FOCUSABLE_SESSIONS) {
    bindings.push(bind(`${MOD}-${index}`, "focus_session", APP, index));
  }
  return bindings;
}

/** The chord bound to an action, for the palette's right-hand column. */
export function chordFor(action: ActionId, bindings = defaultBindings()): Chord | null {
  return bindings.find((binding) => binding.action === action)?.chord ?? null;
}
