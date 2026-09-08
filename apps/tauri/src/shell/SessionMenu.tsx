import { createMemo } from "solid-js";
import { newAgent, newShell } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";
import { openFeatureCompose } from "../store/viewsStore";
import { Icon, SessionGlyph } from "../theme/icons";
import { Menu, type MenuItem } from "../ui";
import { currentWorkspace } from "./sessionActions";

/** The `+` button in the tab strip: start a terminal, an agent, or a feature. */
export function SessionMenu() {
  const agents = createMemo(() => forgeStore.launchables.filter((item) => item.kind === "agent"));

  const items = createMemo<MenuItem[]>(() => {
    const list: MenuItem[] = [
      {
        kind: "item",
        label: "New Terminal",
        glyph: <SessionGlyph providerId={null} size={14} />,
        // Named, not left to the daemon: this menu hangs off the strip,
        // and the strip is one checkout.
        run: () => void newShell(currentWorkspace()).catch(() => undefined),
      },
      {
        kind: "item",
        label: "New Feature…",
        glyph: <Icon name="agent" class="forge-icon-accent" size={14} />,
        run: openFeatureCompose,
      },
      { kind: "rule" },
    ];
    if (agents().length === 0) {
      list.push({
        kind: "item",
        label: "No agent providers in this snapshot.",
        disabled: true,
        run: () => undefined,
      });
      return list;
    }
    for (const item of agents()) {
      list.push({
        kind: "item",
        label: item.label,
        detail: item.enabled ? undefined : "not installed",
        disabled: !item.enabled || !item.provider,
        glyph: item.provider ? (
          <SessionGlyph providerId={item.provider} size={14} />
        ) : (
          <Icon name="agent" class="forge-icon-muted" size={14} />
        ),
        run: () => {
          if (!item.provider) return;
          void newAgent(item.provider, item.profile, currentWorkspace()).catch(() => undefined);
        },
      });
    }
    return list;
  });

  return (
    <Menu
      triggerLabel="New session"
      triggerClass="forge-control forge-control-sm forge-btn forge-btn-ghost forge-icon-button session-menu"
      trigger={<Icon name="plus" class="forge-icon-muted" size={14} />}
      items={items()}
    />
  );
}
