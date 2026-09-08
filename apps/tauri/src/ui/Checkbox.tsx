import { Checkbox as Kobalte } from "@kobalte/core/checkbox";
import { Icon } from "../theme/icons";

export type CheckboxProps = {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  /** Render the label for sighted users too. Off for dense rows. */
  hideLabel?: boolean;
  disabled?: boolean;
  class?: string;
};

/** A checkbox with its label wired to it, which the raw inputs never were. */
export function Checkbox(props: CheckboxProps) {
  return (
    <Kobalte
      class={`forge-checkbox ${props.class ?? ""}`}
      checked={props.checked}
      disabled={props.disabled}
      onChange={props.onChange}
    >
      <Kobalte.Input class="forge-visually-hidden" />
      <Kobalte.Control class="forge-checkbox-box">
        <Kobalte.Indicator>
          <Icon name="check" size={12} />
        </Kobalte.Indicator>
      </Kobalte.Control>
      <Kobalte.Label classList={{ "forge-visually-hidden": props.hideLabel }}>
        {props.label}
      </Kobalte.Label>
    </Kobalte>
  );
}
