import { Separator as Kobalte } from "@kobalte/core/separator";
import { Show, type JSX } from "solid-js";

export type CardProps = {
  children: JSX.Element;
  /** Draws the card selection: an accent edge and an outer halo. */
  selected?: boolean;
  /** Makes the whole card one button; `selected` is then also `aria-pressed`. */
  onClick?: () => void;
  disabled?: boolean;
  "aria-label"?: string;
  class?: string;
};

export function Card(props: CardProps) {
  return (
    <Show
      when={props.onClick}
      fallback={
        <div
          class={`forge-card ${props.class ?? ""}`}
          data-selected={props.selected ? "" : undefined}
        >
          {props.children}
        </div>
      }
    >
      {(onClick) => (
        <button
          type="button"
          class={`forge-card forge-card-button ${props.class ?? ""}`}
          data-selected={props.selected ? "" : undefined}
          aria-pressed={props.selected ?? false}
          aria-label={props["aria-label"]}
          disabled={props.disabled}
          onClick={() => onClick()()}
        >
          {props.children}
        </button>
      )}
    </Show>
  );
}

export type SeparatorProps = {
  orientation?: "horizontal" | "vertical";
  class?: string;
};

export function Separator(props: SeparatorProps) {
  return (
    <Kobalte
      orientation={props.orientation ?? "horizontal"}
      class={`forge-separator ${props.class ?? ""}`}
    />
  );
}
