import { Select as Kobalte } from "@kobalte/core/select";
import { Show, type JSX } from "solid-js";
import { Icon } from "../theme/icons";
import type { ForgeIconName } from "../theme/icons/forgeIcons";

export type SelectOption<T> = {
  value: T;
  label: string;
  disabled?: boolean;
  icon?: ForgeIconName;
  glyph?: JSX.Element;
};

export type SelectProps<T> = {
  value: T | null;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  label?: string;
  placeholder?: string;
  disabled?: boolean;
  class?: string;
  "aria-label"?: string;
};

/**
 * A listbox-backed select.
 *
 * The native `<select>` this replaces could not be styled to the shell's
 * palette on macOS, so the settings pages each had their own escape hatch.
 */
export function Select<T>(props: SelectProps<T>) {
  const selected = () => props.options.find((option) => option.value === props.value) ?? null;
  return (
    <Kobalte<SelectOption<T>>
      class={`forge-select ${props.class ?? ""}`}
      value={selected()}
      options={props.options}
      optionValue="label"
      optionTextValue="label"
      optionDisabled="disabled"
      disabled={props.disabled}
      placeholder={props.placeholder}
      onChange={(option) => {
        if (option) props.onChange(option.value);
      }}
      itemComponent={(itemProps) => (
        <Kobalte.Item item={itemProps.item} class="context-item">
          <span class="context-item-main">
            <Show
              when={itemProps.item.rawValue.glyph}
              fallback={
                <Show when={itemProps.item.rawValue.icon}>
                  {(icon) => <Icon name={icon()} size={14} class="forge-icon-muted" />}
                </Show>
              }
            >
              {(glyph) => glyph()}
            </Show>
            <Kobalte.ItemLabel>{itemProps.item.rawValue.label}</Kobalte.ItemLabel>
          </span>
          <Kobalte.ItemIndicator class="forge-select-check">
            <Icon name="check" size={14} />
          </Kobalte.ItemIndicator>
        </Kobalte.Item>
      )}
    >
      <Show when={props.label}>
        {(label) => <Kobalte.Label class="forge-label">{label()}</Kobalte.Label>}
      </Show>
      <Kobalte.Trigger
        class="forge-control forge-control-md forge-field forge-select-trigger"
        aria-label={props["aria-label"]}
      >
        <Kobalte.Value<SelectOption<T>>>
          {(state) => {
            const option = state.selectedOption();
            return (
              <span class="forge-select-value">
                <Show
                  when={option.glyph}
                  fallback={
                    <Show when={option.icon}>
                      {(icon) => <Icon name={icon()} size={14} class="forge-icon-muted" />}
                    </Show>
                  }
                >
                  {(glyph) => glyph()}
                </Show>
                <span>{option.label}</span>
              </span>
            );
          }}
        </Kobalte.Value>
        <Kobalte.Icon class="forge-select-icon">
          <Icon name="chevron-down" class="forge-icon-muted" size={14} />
        </Kobalte.Icon>
      </Kobalte.Trigger>
      <Kobalte.Portal>
        <Kobalte.Content class="context-menu forge-select-content">
          <Kobalte.Listbox />
        </Kobalte.Content>
      </Kobalte.Portal>
    </Kobalte>
  );
}
