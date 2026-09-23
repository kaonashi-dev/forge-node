import { RadioGroup as Kobalte } from "@kobalte/core/radio-group";
import { Index, Show, type JSX } from "solid-js";

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
  /**
   * `list` draws a dot beside each label; `chips` and `segmented` draw the
   * label alone, as pills or as one segmented track, and lay out in a row.
   */
  variant?: "list" | "chips" | "segmented";
  class?: string;
  itemClass?: string;
};

const GROUP_CLASS = { list: "", chips: "filter-chips", segmented: "forge-segmented" } as const;
const ITEM_CLASS = { list: "", chips: "forge-chip", segmented: "forge-segment" } as const;

/**
 * A single choice out of several.
 *
 * The rows this replaces were `<label><input type="radio">` pairs, which meant
 * Tab stopped on every option instead of on the group. Kobalte gives the group
 * one tab stop and moves between options with the arrow keys.
 */
export function RadioGroup(props: RadioGroupProps) {
  const variant = () => props.variant ?? "list";
  const bare = () => variant() !== "list";
  return (
    <Kobalte
      class={`forge-radio-group ${GROUP_CLASS[variant()]} ${props.class ?? ""}`}
      value={props.value ?? undefined}
      orientation={props.orientation ?? (bare() ? "horizontal" : "vertical")}
      onChange={props.onChange}
    >
      <Kobalte.Label classList={{ "forge-visually-hidden": props.hideLabel !== false }}>
        {props.label}
      </Kobalte.Label>
      {/*
       * `Index`, not `For`.
       *
       * An option list is positional and short, and callers build theirs inside
       * the JSX — a filter strip's array is rebuilt every time the selected
       * value changes, because the selected value is in it. Keyed on identity
       * that means every option is torn down and remounted on each click: the
       * transition never plays (the element it would have played on is gone),
       * and the pointer lands on a node that was not there when it was pressed.
       * Keyed on position the items survive, and Kobalte reads `value`,
       * `disabled` and the label reactively, so the contents still follow.
       */}
      <Index each={props.options}>
        {(option) => (
          <Kobalte.Item
            value={option().value}
            disabled={option().disabled}
            class={`forge-radio ${ITEM_CLASS[variant()]} ${props.itemClass ?? ""}`}
          >
            <Kobalte.ItemInput class="forge-visually-hidden" />
            <Show
              when={option().render}
              fallback={
                <Show
                  when={!bare()}
                  fallback={<Kobalte.ItemLabel>{option().label}</Kobalte.ItemLabel>}
                >
                  <Kobalte.ItemControl class="forge-radio-dot">
                    <Kobalte.ItemIndicator />
                  </Kobalte.ItemControl>
                  <Kobalte.ItemLabel class="settings-row-label">{option().label}</Kobalte.ItemLabel>
                  <Show when={option().note}>
                    {(note) => (
                      <Kobalte.ItemDescription class="settings-row-note">
                        {note()}
                      </Kobalte.ItemDescription>
                    )}
                  </Show>
                </Show>
              }
            >
              {(render) => (
                <Kobalte.ItemLabel class="forge-radio-custom">{render()()}</Kobalte.ItemLabel>
              )}
            </Show>
          </Kobalte.Item>
        )}
      </Index>
    </Kobalte>
  );
}
