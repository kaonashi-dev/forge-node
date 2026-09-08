import { Collapsible } from "@kobalte/core/collapsible";
import type { JSX } from "solid-js";
import { Icon } from "../theme/icons";

export type DisclosureProps = {
  /** The always-visible row. Rendered inside the trigger, so keep it flat. */
  summary: JSX.Element;
  children: JSX.Element;
  /** Controls that must stay clickable — they sit beside the trigger, not in it. */
  actions?: JSX.Element;
  defaultOpen?: boolean;
  class?: string;
};

/**
 * A row that opens to show its detail.
 *
 * `actions` is a separate slot on purpose: a button nested inside a trigger is
 * a button whose click also toggles the row, and a "Set default" that collapses
 * what you were reading is worse than no shortcut at all.
 */
export function Disclosure(props: DisclosureProps) {
  return (
    <Collapsible class={`forge-disclosure ${props.class ?? ""}`} defaultOpen={props.defaultOpen}>
      <div class="forge-disclosure-head">
        <Collapsible.Trigger class="forge-disclosure-trigger">{props.summary}</Collapsible.Trigger>
        {props.actions}
        <Collapsible.Trigger class="forge-disclosure-chevron" aria-label="Show details">
          <Icon name="chevron-down" class="forge-icon-muted" size={14} />
        </Collapsible.Trigger>
      </div>
      <Collapsible.Content class="forge-disclosure-body">{props.children}</Collapsible.Content>
    </Collapsible>
  );
}
