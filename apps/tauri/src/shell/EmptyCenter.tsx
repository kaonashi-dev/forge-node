import { For, Show } from "solid-js";
import type { ActionId } from "../actions/actions";
import { boundChord } from "../actions/bindings";
import { invokeAction } from "../actions/dispatch";
import { Kbd } from "../ui";
import { forgeStore } from "../store/forgeStore";
import { Icon, type ForgeIconName } from "../theme/icons";

/**
 * What the centre column shows when there is no session to show.
 *
 * The terminal pane paints the last frame it was sent and keeps painting it,
 * so a window whose last session just closed went on showing that session's
 * scrollback — a dead terminal that reads exactly like a live one, down to the
 * grid size in the status bar. The only thing separating them was a toast.
 *
 * Every row here goes back through the dispatcher rather than calling the
 * runtime itself: these are second ways in to the actions the menus and the
 * palette already have, not second implementations of them, and the chords
 * are read off the keymap so they cannot drift from what the keys do.
 *
 * The rows are not gated on the actions being registered. `actionIsBound`
 * reads a plain map rather than a signal, and the shell registers in
 * `onMount` — which runs after its children render — so a window opened with
 * no sessions would have shown this card with every row hidden. The shell
 * binds these for its whole life, and `invokeAction` is a no-op if it ever
 * did not.
 */
export function EmptyCenter() {
  const bare = () => forgeStore.projects.length === 0;

  const rows = (): { action: ActionId; icon: ForgeIconName; label: string; note: string }[] =>
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
    <div class="empty-center">
      <div class="empty-center-card">
        <p class="empty-center-title">No sessions open</p>
        <p class="empty-center-copy">
          {bare()
            ? "Nothing is set up yet."
            : "Everything here is closed. Start one to get back to work."}
        </p>

        <ul class="empty-center-rows">
          <For each={rows()}>
            {(row) => (
              <li>
                <button
                  type="button"
                  class="forge-row empty-center-row"
                  onClick={() => invokeAction(row.action)}
                >
                  <Icon name={row.icon} class="forge-icon-muted" size={15} />
                  <span class="empty-center-label">{row.label}</span>
                  <span class="empty-center-note">{row.note}</span>
                  <Hint action={row.action} />
                </button>
              </li>
            )}
          </For>
        </ul>

        <p class="empty-center-foot">
          Everything else is behind <Hint action="open_command_palette" />
        </p>
      </div>
    </div>
  );
}

function Hint(props: { action: ActionId }) {
  const chord = () => boundChord(props.action);
  return (
    <Show when={chord()}>{(found) => <Kbd chord={found()} class="empty-center-chord" />}</Show>
  );
}
