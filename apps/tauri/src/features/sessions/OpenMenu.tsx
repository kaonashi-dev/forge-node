import { Show, createMemo, type JSX } from "solid-js";
import { openInEditor, openInFileManager } from "../../runtime/host";
import { setAppState } from "../settings/commands";
import { forgeStore } from "../../state/forgeStore";
import { connectionStore } from "../../state/connection";
import { BrandIcon, Icon } from "../../theme/icons/index";
import { Button, Menu, Tooltip, toast, type MenuItem } from "../../ui/index";
import {
  OPEN_WITH_KEY,
  openTargetPath,
  openTargets,
  preferredTarget,
  type OpenTarget,
} from "./openWith";
import { activeWorkspace } from "../../state/workspace";

function glyph(target: OpenTarget): JSX.Element {
  if (target.id === "zed") {
    return <BrandIcon brand="editor-zed" size={14} class="forge-icon-muted" />;
  }
  if (target.id === "cursor") {
    return <BrandIcon brand="cursor" size={14} class="forge-icon-muted" />;
  }
  return <Icon name="folder-open" size={14} class="forge-icon-muted" />;
}

/**
 * "Open" in the title bar: a click runs the last target, the chevron picks one.
 *
 * The rail's own "Open in" submenu stays — it acts on whichever row was
 * right-clicked, while this one always acts on the checkout being looked at.
 */
export function OpenMenu() {
  const path = createMemo(() =>
    openTargetPath(forgeStore, connectionStore.activeSession, activeWorkspace()),
  );
  const target = createMemo(() => preferredTarget(forgeStore.app_state));

  function open(next: OpenTarget): void {
    const dir = path();
    if (!dir) return;
    // Remembered before the launch, not after: "which one did I pick last" is
    // answered by the pick, and an editor that is not installed still made one.
    void setAppState(OPEN_WITH_KEY, next.id).catch(() => undefined);
    const launch = next.editor ? openInEditor(next.editor, dir) : openInFileManager(dir);
    void launch.catch((error: unknown) =>
      toast({ tone: "danger", title: `Could not open ${next.label}`, detail: String(error) }),
    );
  }

  const items = createMemo<MenuItem[]>(() =>
    openTargets().map((entry) => ({
      kind: "item",
      label: entry.label,
      detail: entry.id === target().id ? "default" : undefined,
      glyph: glyph(entry),
      run: () => open(entry),
    })),
  );

  return (
    <Show when={path()}>
      <div class="open-split">
        <Tooltip label={`Open this checkout in ${target().label}`} contents>
          <Button
            variant="ghost"
            size="sm"
            class="open-split-main"
            iconLeading={glyph(target())}
            onClick={() => open(target())}
          >
            Open
          </Button>
        </Tooltip>
        <Menu
          triggerLabel="Open with…"
          triggerClass="forge-control forge-control-sm forge-btn forge-btn-ghost open-split-more"
          trigger={<Icon name="chevron-down" class="forge-icon-muted" size={14} />}
          items={items()}
        />
      </div>
    </Show>
  );
}
