import { For } from "solid-js";
import type { ActionId } from "../../actions/actions";
import { invokeAction } from "../../actions/dispatch";
import { Kbd } from "../../ui/index";
import { forgeStore } from "../../state/forgeStore";
import { canReopenClosed } from "../../navigation/viewsStore";
import { Icon, type ForgeIconName } from "../../theme/icons/index";

/**
 * What the centre column shows when there is no session to show.
 *
 * The terminal pane keeps painting its last frame, so without this card a
 * closed session would read exactly like a live one.
 *
 * Rows go through the dispatcher, and their chords are read off the keymap, so
 * they cannot drift from the menus and the palette. They are not gated on the
 * actions being registered: the shell registers in `onMount`, after its
 * children render, and `invokeAction` is a no-op for an unbound action.
 */
export function EmptyCenter() {
  const bare = () => forgeStore.projects.length === 0;

  const rows = (): EmptyRow[] =>
    bare()
      ? [
          {
            action: "add_project",
            icon: "folder-open",
            label: "Add a project",
            note: "Point Forge Node at a repository to work in",
          },
        ]
      : [
          {
            action: "new_terminal",
            icon: "square-terminal",
            label: "New terminal",
            note: "A shell in the current checkout",
          },
          {
            action: "new_agent",
            icon: "agent",
            label: "New agent",
            note: "Start a coding agent in a session",
          },
          {
            action: "new_worktree",
            icon: "git-branch",
            label: "New worktree",
            note: "Checkout a branch in a new folder",
          },
          {
            action: "add_project",
            icon: "folder-open",
            label: "Add a project",
            note: "Work in another repository",
          },
        ];

  return (
    <EmptyCard
      title="No sessions open"
      copy={
        bare()
          ? "Nothing is set up yet."
          : "Everything here is closed. Start one to get back to work."
      }
      rows={rows()}
    />
  );
}

const CODE_ROWS: EmptyRow[] = [
  {
    action: "open_file_palette",
    icon: "file-code",
    label: "Open file",
    note: "Jump to any file in the checkout",
  },
  {
    action: "find_in_project",
    icon: "search",
    label: "Find in project",
    note: "Search the checkout's contents",
  },
  {
    action: "toggle_files",
    icon: "folder",
    label: "Browse files",
    note: "Show the file tree in the sidebar",
  },
];

const REOPEN_ROW: EmptyRow = {
  action: "reopen_closed_tab",
  icon: "folder-open",
  label: "Reopen closed tab",
  note: "Bring back the last view you closed",
};

/** What Code shows before any file is open. */
export function EmptyCode() {
  return (
    <EmptyCard
      title="No files open"
      copy="Open a file to start working in Code."
      rows={canReopenClosed() ? [...CODE_ROWS, REOPEN_ROW] : CODE_ROWS}
    />
  );
}

type EmptyRow = { action: ActionId; icon: ForgeIconName; label: string; note: string };

function EmptyCard(props: { title: string; copy: string; rows: EmptyRow[] }) {
  return (
    <div class="empty-center">
      <div class="empty-center-card">
        <p class="forge-card-title empty-center-title">{props.title}</p>
        <p class="empty-center-copy">{props.copy}</p>

        <ul class="empty-state-rows">
          <For each={props.rows}>
            {(row) => (
              <li>
                <button
                  type="button"
                  class="forge-row empty-state-row"
                  onClick={() => invokeAction(row.action)}
                >
                  <Icon name={row.icon} class="forge-icon-muted" size={15} />
                  <span class="empty-center-label">{row.label}</span>
                  <span class="forge-truncate empty-center-note">{row.note}</span>
                  <Kbd action={row.action} />
                </button>
              </li>
            )}
          </For>
        </ul>

        <p class="empty-center-foot">
          Everything else is behind <Kbd action="open_command_palette" />
        </p>
      </div>
    </div>
  );
}
