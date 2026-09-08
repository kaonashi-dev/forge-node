import { Switch as Kobalte } from "@kobalte/core/switch";
import { Show } from "solid-js";
import type { ControlSize } from "./types";

export type SwitchProps = {
  /** Announced name. Rendered unless `hideLabel` is set. */
  label: string;
  hideLabel?: boolean;
  description?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  size?: ControlSize;
  class?: string;
};

/**
 * A boolean setting (§5.3).
 *
 * One answer for a question that had three in `settings/`: a pill button that
 * looked pressed, a checkbox, and a pair of radio chips reading "on"/"off".
 * A switch is the right one because the change takes effect immediately —
 * a checkbox promises a form to submit, and these settings have none.
 */
export function Switch(props: SwitchProps) {
  return (
    <Kobalte
      class={`forge-switch forge-switch-${props.size ?? "md"} ${props.class ?? ""}`}
      checked={props.checked}
      onChange={props.onChange}
      disabled={props.disabled}
    >
      <Show when={!props.hideLabel}>
        <div class="forge-switch-text">
          <Kobalte.Label class="forge-switch-label">{props.label}</Kobalte.Label>
          <Show when={props.description}>
            {(text) => <Kobalte.Description class="forge-hint">{text()}</Kobalte.Description>}
          </Show>
        </div>
      </Show>
      <Show when={props.hideLabel}>
        <Kobalte.Label class="forge-visually-hidden">{props.label}</Kobalte.Label>
      </Show>
      <Kobalte.Input class="forge-switch-input" />
      <Kobalte.Control class="forge-switch-track">
        <Kobalte.Thumb class="forge-switch-thumb" />
      </Kobalte.Control>
    </Kobalte>
  );
}
