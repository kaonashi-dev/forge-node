import { Show, createMemo, createSignal } from "solid-js";
import { close } from "../../../features/editor/tabs";
import { focus } from "../../../navigation/viewsStore";
import { sameView, viewLabel, viewTitle, type WorkbenchView } from "../../../navigation/views";
import { forgeStore } from "../../../state/forgeStore";
import { Icon } from "../../../theme/icons/index";
import { IconButton } from "../../../ui/index";
import { ViewGlyph } from "./ViewGlyph";

/**
 * One strip tab in Code.
 *
 * Dirty tracks `Session.editor` through a memo — the same source as the
 * status bar's "unsaved" mark — so the close control can yield to the dot.
 */
export function WorkbenchTab(props: {
  view: WorkbenchView;
  active: WorkbenchView;
  onMenu: (event: MouseEvent, view: WorkbenchView) => void;
}) {
  const dirty = createMemo(() => {
    const view = props.view;
    if (view.kind !== "editor-terminal") return false;
    return (
      forgeStore.sessions.find((session) => session.id === view.session)?.editor?.dirty === true
    );
  });
  // Dot by default when dirty; the close glyph only while the control is armed.
  const [closing, setClosing] = createSignal(false);
  const showClose = () => !dirty() || closing();

  return (
    <div
      class="workbench-tab"
      classList={{
        active: sameView(props.active, props.view),
        dirty: dirty(),
      }}
      role="tab"
      aria-selected={sameView(props.active, props.view)}
      onContextMenu={(event) => {
        event.preventDefault();
        props.onMenu(event, props.view);
      }}
      // Middle-click closes, the way it does in every browser. On `auxclick`
      // and not `mouseup`: `mouseup` fires for the right button too, and would
      // close the tab a context menu is opening on.
      onAuxClick={(event) => {
        if (event.button !== 1) return;
        event.preventDefault();
        close(props.view);
      }}
    >
      <button
        type="button"
        class="forge-row workbench-tab-label"
        aria-label={viewTitle(props.view)}
        onClick={() => focus(props.view)}
      >
        <ViewGlyph view={props.view} />
        <span class="workbench-tab-text">{viewLabel(props.view)}</span>
      </button>
      <span
        class="workbench-tab-close-hit"
        onMouseEnter={() => setClosing(true)}
        onMouseLeave={() => setClosing(false)}
      >
        <IconButton
          label={`Close ${viewLabel(props.view)}`}
          hideTooltip
          size="xs"
          class="workbench-tab-close"
          onClick={() => close(props.view)}
        >
          <Show
            when={showClose()}
            fallback={<span class="workbench-tab-dirty" aria-hidden="true" />}
          >
            <Icon name="close" class="workbench-tab-close-icon" size={12} />
          </Show>
        </IconButton>
      </span>
    </div>
  );
}
