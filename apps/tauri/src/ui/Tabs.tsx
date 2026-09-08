import { Tabs as Kobalte } from "@kobalte/core/tabs";
import { For, type JSX } from "solid-js";

export type TabDef = {
  value: string;
  label: string | JSX.Element;
  content: () => JSX.Element;
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
              <Kobalte.Trigger value={tab.value} class={props.triggerClass}>
                {tab.label}
              </Kobalte.Trigger>
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
