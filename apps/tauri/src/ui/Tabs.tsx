import { Tabs as Kobalte } from "@kobalte/core/tabs";
import { For, Show, type JSX } from "solid-js";
import { Icon, type ForgeIconName } from "../theme/icons";
import { Tooltip } from "./Tooltip";

export type TabDef = {
  value: string;
  label: string | JSX.Element;
  content: () => JSX.Element;
  /**
   * Draws the trigger as a glyph. `label` must then be a plain string: it
   * becomes the trigger's accessible name and its tooltip, and an icon alone
   * says nothing to a screen reader.
   */
  icon?: ForgeIconName;
};

export type TabsProps = {
  value: string;
  tabs: TabDef[];
  onChange: (value: string) => void;
  orientation?: "horizontal" | "vertical";
  class?: string;
  listClass?: string;
  triggerClass?: string;
  contentClass?: string;
  /** Rendered after the triggers, inside the list's container. */
  listSuffix?: JSX.Element;
  "aria-label"?: string;
};

/**
 * A set of panels with one visible at a time.
 *
 * Worth the primitive for the roving tab stop alone: a rail of plain buttons
 * puts every section in the Tab order and tells assistive tech nothing about
 * which panel each one reveals.
 */
export function Tabs(props: TabsProps) {
  return (
    <Kobalte
      class={props.class}
      aria-label={props["aria-label"]}
      value={props.value}
      orientation={props.orientation ?? "horizontal"}
      onChange={props.onChange}
    >
      <div class={props.listClass}>
        <Kobalte.List class="forge-tab-list">
          <For each={props.tabs}>
            {(tab) => (
              <Show
                when={tab.icon}
                fallback={
                  <Kobalte.Trigger value={tab.value} class={props.triggerClass}>
                    {tab.label}
                  </Kobalte.Trigger>
                }
              >
                {/* `contents` keeps the trigger a direct child of the list in
                    the accessibility tree: a generic element between
                    `role="tablist"` and `role="tab"` is the one ARIA rule a tab
                    strip cannot bend. */}
                {(icon) => (
                  <Tooltip label={String(tab.label)} contents>
                    <Kobalte.Trigger
                      value={tab.value}
                      class={`${props.triggerClass ?? ""} forge-tab-icon`}
                      aria-label={String(tab.label)}
                    >
                      <Icon name={icon()} size={16} />
                    </Kobalte.Trigger>
                  </Tooltip>
                )}
              </Show>
            )}
          </For>
        </Kobalte.List>
        {props.listSuffix}
      </div>
      <For each={props.tabs}>
        {(tab) => (
          <Kobalte.Content value={tab.value} class={props.contentClass}>
            {tab.content()}
          </Kobalte.Content>
        )}
      </For>
    </Kobalte>
  );
}
