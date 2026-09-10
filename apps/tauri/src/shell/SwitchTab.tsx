import { For, Show, createEffect, onCleanup, onMount } from "solid-js";
import { COMMAND_PALETTE } from "../actions/actions";
import { enterContext } from "../actions/dispatch";
import { sessionTabLabel } from "../runtime/attention";
import type { Project, Session, Workspace } from "../runtime/types";
import { Icon, SessionGlyph } from "../theme/icons";
import { Dialog } from "../ui";
import {
  cancelTabSwitcher,
  chooseTabSwitcher,
  hoverTabSwitcher,
  nudgeTabSwitcher,
  type TabSwitcherView,
} from "./tabSwitcher";

const VISIBLE_ROWS = 10;

export type TabSwitcherProps = {
  view: TabSwitcherView;
  sessions: readonly Session[];
  workspaces: readonly Workspace[];
  projects: readonly Project[];
};

/**
 * Hold-Control tab list. No query field: Tab / Shift+Tab move the cursor while
 * Control is down, and releasing Control focuses the highlighted row.
 */
export function TabSwitcher(props: TabSwitcherProps) {
  let list: HTMLDivElement | undefined;

  createEffect(() => {
    const index = props.view.index;
    const node = list?.querySelector<HTMLElement>(`[data-switcher-index="${index}"]`);
    node?.scrollIntoView({ block: "nearest" });
  });

  onMount(() => {
    onCleanup(enterContext(COMMAND_PALETTE));
    list?.focus();
  });

  return (
    <Dialog
      title="Switch Tab"
      flush
      size="sm"
      align="top"
      class="tab-switcher"
      onDismiss={cancelTabSwitcher}
    >
      <div
        ref={list}
        class="palette-list tab-switcher-list"
        role="listbox"
        tabindex={0}
        aria-label="Switch Tab"
        style={{ "--palette-rows": VISIBLE_ROWS }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            nudgeTabSwitcher(1);
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            nudgeTabSwitcher(-1);
          } else if (event.key === "Enter") {
            event.preventDefault();
            const id = props.view.ids[props.view.index];
            if (id) chooseTabSwitcher(id);
          }
        }}
      >
        <For each={props.view.ids}>
          {(id, index) => {
            const session = () => props.sessions.find((item) => item.id === id);
            const selected = () => index() === props.view.index;
            const foreign = () => index() >= props.view.foreignAt;
            const peers = () => {
              const row = session();
              if (!row) return [] as Session[];
              return props.sessions.filter((item) => item.workspace_id === row.workspace_id);
            };
            const label = () => {
              const row = session();
              return row ? sessionTabLabel(row, peers()) : id;
            };
            const note = () => {
              const row = session();
              if (!row || !foreign()) return null;
              return foreignNote(row, props.workspaces, props.projects);
            };
            const provider = () => session()?.agent_provider_id ?? null;
            return (
              <>
                <Show
                  when={
                    index() === props.view.foreignAt && props.view.foreignAt < props.view.ids.length
                  }
                >
                  <div class="palette-group">Recent</div>
                </Show>
                <button
                  type="button"
                  role="option"
                  data-switcher-index={index()}
                  class="palette-row"
                  classList={{ selected: selected() }}
                  aria-selected={selected()}
                  onClick={() => chooseTabSwitcher(id)}
                  onMouseEnter={() => hoverTabSwitcher(index())}
                >
                  {provider() ? (
                    <span class="palette-glyph">
                      <SessionGlyph providerId={provider()!} emphasis="dim" size={14} />
                    </span>
                  ) : (
                    <Icon name="square-terminal" class="forge-icon-faint palette-glyph" size={14} />
                  )}
                  <span class="palette-label">{label()}</span>
                  <Show when={note()}>{(text) => <span class="palette-note">{text()}</span>}</Show>
                </button>
              </>
            );
          }}
        </For>
      </div>
    </Dialog>
  );
}

/** Where a foreign row lives: project name, else the checkout's branch. */
function foreignNote(
  session: Session,
  workspaces: readonly Workspace[],
  projects: readonly Project[],
): string | null {
  const workspace = workspaces.find((item) => item.id === session.workspace_id);
  if (!workspace) return null;
  const project = projects.find((item) => item.id === workspace.project_id);
  if (project?.name) return project.name;
  return workspace.branch ?? workspace.display_name ?? null;
}
