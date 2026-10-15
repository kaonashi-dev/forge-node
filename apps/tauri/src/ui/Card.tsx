import { Separator as Kobalte } from "@kobalte/core/separator";
import type { JSX } from "solid-js";
import { Dynamic } from "solid-js/web";

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
  const interactive = () => props.onClick !== undefined;
  return (
    <Dynamic
      component={interactive() ? "button" : "div"}
      type={interactive() ? "button" : undefined}
      class={`forge-card ${interactive() ? "forge-card-button" : ""} ${props.class ?? ""}`}
      data-selected={props.selected ? "" : undefined}
      aria-pressed={interactive() ? (props.selected ?? false) : undefined}
      aria-label={props["aria-label"]}
      disabled={interactive() ? props.disabled : undefined}
      onClick={props.onClick}
    >
      {props.children}
    </Dynamic>
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
