import { For, Show, createEffect, onCleanup, onMount } from "solid-js";
import { COMMAND_PALETTE } from "../../../actions/actions";
import { enterContext } from "../../../actions/dispatch";
import { sessionTabLabel } from "../../../features/sessions/attention";
import type { Project, Session, Workspace } from "../../../contracts/runtime";
import { SessionGlyph } from "../../../features/sessions/SessionGlyph";
import { Icon } from "../../../theme/icons/index";
import { Dialog } from "../../../ui/index";
import { viewLabel, type WorkbenchView } from "../../../navigation/views";
import { parentPath } from "../../../shared/paths";
import {
  cancelTabSwitcher,
  chooseTabSwitcher,
  commitTabSwitcher,
  hoverTabSwitcher,
  nudgeTabSwitcher,
  pinTabSwitcher,
  releaseTabSwitcher,
  type TabSwitcherView,
} from "../../../navigation/tabSwitcher";
import { ViewGlyph } from "./ViewGlyph";

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
 *
 * Moving the pointer over the card hands it to the mouse instead — Control can
 * then be released and a row clicked; taking the pointer back off it returns
 * the release to the keyboard.
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
    // The whole card, not the rows: a hand reaching for the mouse crosses the
    // title and the padding too, and which pixel it crossed must not decide
    // whether letting go of Control commits or leaves the list up.
    const card = list?.closest(".forge-dialog-card");
    if (!card) return;
    card.addEventListener("pointermove", pinTabSwitcher);
    card.addEventListener("pointerleave", releaseTabSwitcher);
    onCleanup(() => {
      card.removeEventListener("pointermove", pinTabSwitcher);
      card.removeEventListener("pointerleave", releaseTabSwitcher);
    });
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
            commitTabSwitcher();
          }
        }}
      >
        <For each={props.view.targets}>
          {(target, index) => {
            const selected = () => index() === props.view.index;
            const foreign = () => index() >= props.view.foreignAt;
            return (
              <>
                <Show
                  when={
                    index() === props.view.foreignAt &&
                    props.view.foreignAt < props.view.targets.length
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
                  onClick={() => chooseTabSwitcher(index())}
                  onMouseEnter={() => hoverTabSwitcher(index())}
                >
                  {target.kind === "code" ? (
                    <CodeRow />
                  ) : target.kind === "view" ? (
                    <ViewRow view={target.view} />
                  ) : (
                    <SessionRow
                      id={target.id}
                      foreign={foreign()}
                      sessions={props.sessions}
                      workspaces={props.workspaces}
                      projects={props.projects}
                    />
                  )}
                </button>
              </>
            );
          }}
        </For>
      </div>
    </Dialog>
  );
}

/** The Code tab itself, the way the strip shows it while nothing is parked in it. */
function CodeRow() {
  return (
    <>
      <Icon name="folder-open" class="forge-icon-muted palette-glyph" size={14} />
      <span class="palette-label">Code</span>
    </>
  );
}

/** A file, diff or pull request parked in Code. */
function ViewRow(props: { view: WorkbenchView }) {
  const note = () => {
    const view = props.view;
    if (view.kind !== "editor-terminal" && view.kind !== "preview") return null;
    return parentPath(view.path) || null;
  };
  return (
    <>
      <span class="palette-glyph">
        <ViewGlyph view={props.view} size={14} />
      </span>
      <span class="palette-label">{viewLabel(props.view)}</span>
      <Show when={note()}>{(text) => <span class="palette-note">{text()}</span>}</Show>
    </>
  );
}

function SessionRow(props: {
  id: string;
  foreign: boolean;
  sessions: readonly Session[];
  workspaces: readonly Workspace[];
  projects: readonly Project[];
}) {
  const session = () => props.sessions.find((item) => item.id === props.id);
  const peers = () => {
    const row = session();
    if (!row) return [] as Session[];
    return props.sessions.filter((item) => item.workspace_id === row.workspace_id);
  };
  const label = () => {
    const row = session();
    return row ? sessionTabLabel(row, peers()) : props.id;
  };
  const note = () => {
    const row = session();
    if (!row || !props.foreign) return null;
    return foreignNote(row, props.workspaces, props.projects);
  };
  const provider = () => session()?.agent_provider_id ?? null;
  return (
    <>
      {provider() ? (
        <span class="palette-glyph">
          <SessionGlyph providerId={provider()!} emphasis="dim" size={14} />
        </span>
      ) : (
        <Icon name="square-terminal" class="forge-icon-faint palette-glyph" size={14} />
      )}
      <span class="palette-label">{label()}</span>
      <Show when={note()}>{(text) => <span class="palette-note">{text()}</span>}</Show>
    </>
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
