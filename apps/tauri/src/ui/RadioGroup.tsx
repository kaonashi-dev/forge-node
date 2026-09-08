import { RadioGroup as Kobalte } from "@kobalte/core/radio-group";
import { For, Show, type JSX } from "solid-js";

export type RadioOption = {
  /** Stable string key. Callers that hold richer values map through it. */
  value: string;
  label: string;
  note?: string | null;
  disabled?: boolean;
  /** Replaces the label row entirely, for swatches and other rich choices. */
  render?: () => JSX.Element;
};

export type RadioGroupProps = {
  value: string | null;
  options: RadioOption[];
  onChange: (value: string) => void;
  label: string;
  hideLabel?: boolean;
  orientation?: "horizontal" | "vertical";
  class?: string;
  itemClass?: string;
};

/**
 * A single choice out of several.
 *
 * The rows this replaces were `<label><input type="radio">` pairs, which meant
 * Tab stopped on every option instead of on the group. Kobalte gives the group
 * one tab stop and moves between options with the arrow keys.
 */
export function RadioGroup(props: RadioGroupProps) {
  return (
    <Kobalte
      class={`forge-radio-group ${props.class ?? ""}`}
      value={props.value ?? undefined}
      orientation={props.orientation ?? "vertical"}
      onChange={props.onChange}
    >
      <Kobalte.Label classList={{ "forge-visually-hidden": props.hideLabel !== false }}>
        {props.label}
      </Kobalte.Label>
      <For each={props.options}>
        {(option) => (
          <Kobalte.Item
            value={option.value}
            disabled={option.disabled}
            class={`forge-radio ${props.itemClass ?? ""}`}
          >
            <Kobalte.ItemInput class="forge-visually-hidden" />
            <Show
              when={option.render}
              fallback={
                <>
                  <Kobalte.ItemControl class="forge-radio-dot">
                    <Kobalte.ItemIndicator />
                  </Kobalte.ItemControl>
                  <Kobalte.ItemLabel class="settings-row-label">{option.label}</Kobalte.ItemLabel>
                  <Show when={option.note}>
                    {(note) => (
                      <Kobalte.ItemDescription class="settings-row-note">
                        {note()}
                      </Kobalte.ItemDescription>
                    )}
                  </Show>
                </>
              }
            >
              {(render) => (
                <Kobalte.ItemLabel class="forge-radio-custom">{render()()}</Kobalte.ItemLabel>
              )}
            </Show>
          </Kobalte.Item>
        )}
      </For>
    </Kobalte>
  );
}
