import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { For, Show, type JSX } from "solid-js";
import { Icon } from "../theme/icons";
import type { MenuItem } from "./types";

export type MenuProps = {
  /** The button that opens the menu. */
  trigger: JSX.Element;
  triggerClass?: string;
  triggerLabel: string;
  items: MenuItem[];
  placement?: "bottom-start" | "bottom-end" | "top-start" | "top-end";
};

/**
 * A menu hanging off a button.
 *
 * Kobalte owns the parts the two hand-rolled menus never had: arrow-key
 * navigation, typeahead, `aria-activedescendant`, focus returned to the
 * trigger on close, and dismissal that does not also activate whatever was
 * behind the menu.
 */
export function Menu(props: MenuProps) {
  return (
    <DropdownMenu placement={props.placement ?? "bottom-end"} gutter={4}>
      <DropdownMenu.Trigger class={props.triggerClass} aria-label={props.triggerLabel}>
        {props.trigger}
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="context-menu">
          <MenuItems items={props.items} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

export type ContextMenuProps = {
  /** Viewport coordinates the menu is anchored to. */
  x: number;
  y: number;
  items: MenuItem[];
  onDismiss: () => void;
};

/**
 * A menu at a point, for right-clicks.
 *
 * Anchored with `getAnchorRect` rather than a trigger element: the anchor is
 * the pointer, which has no DOM node. Everything else — flipping near the
 * window edge, keyboard nav, dismissal — comes from the same primitive the
 * button menus use.
 */
export function ContextMenu(props: ContextMenuProps) {
  return (
    <DropdownMenu
      open
      onOpenChange={(open) => {
        if (!open) props.onDismiss();
      }}
      placement="bottom-start"
      getAnchorRect={() => ({ x: props.x, y: props.y, width: 0, height: 0 })}
    >
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="context-menu">
          <MenuItems items={props.items} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

function MenuItems(props: { items: MenuItem[] }) {
  return (
    <For each={props.items}>
      {(item) => {
        if (item.kind === "rule") return <DropdownMenu.Separator class="context-rule" />;
        if (item.kind === "heading") {
          // Not `DropdownMenu.GroupLabel`: that one throws unless it is wrapped
          // in a `Group`, and these headings label a run of items in a flat
          // list rather than a group with a boundary.
          return (
            <div class="context-heading" role="presentation">
              {item.label}
            </div>
          );
        }
        if (item.kind === "submenu") {
          return (
            <DropdownMenu.Sub overlap gutter={-1} shift={-5}>
              <DropdownMenu.SubTrigger class="context-item">
                <span class="context-item-main">
                  <Show when={item.icon}>
                    {(icon) => <Icon name={icon()} class="forge-icon-muted" size={14} />}
                  </Show>
                  <span>{item.label}</span>
                </span>
                <Icon name="chevron-right" class="forge-icon-muted" size={14} />
              </DropdownMenu.SubTrigger>
              <DropdownMenu.Portal>
                <DropdownMenu.SubContent class="context-menu">
                  <MenuItems items={item.items} />
                </DropdownMenu.SubContent>
              </DropdownMenu.Portal>
            </DropdownMenu.Sub>
          );
        }
        return (
          <DropdownMenu.Item
            class="context-item"
            classList={{ destructive: item.destructive }}
            disabled={item.disabled}
            onSelect={item.run}
          >
            <span class="context-item-main">
              <Show
                when={item.glyph}
                fallback={
                  <Show when={item.icon}>
                    {(icon) => (
                      <Icon
                        name={icon()}
                        class={item.destructive ? "forge-icon-red" : "forge-icon-muted"}
                        size={14}
                      />
                    )}
                  </Show>
                }
              >
                {item.glyph}
              </Show>
              <DropdownMenu.ItemLabel>{item.label}</DropdownMenu.ItemLabel>
            </span>
            <Show when={item.detail}>
              {(detail) => (
                <DropdownMenu.ItemDescription class="context-detail">
                  {detail()}
                </DropdownMenu.ItemDescription>
              )}
            </Show>
          </DropdownMenu.Item>
        );
      }}
    </For>
  );
}
