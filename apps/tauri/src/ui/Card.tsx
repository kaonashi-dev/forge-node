import { Separator as Kobalte } from "@kobalte/core/separator";
import type { JSX } from "solid-js";

export type CardProps = {
  children: JSX.Element;
  /** Draws the card selection: an accent edge and an outer halo. */
  selected?: boolean;
  class?: string;
};

export function Card(props: CardProps) {
  return (
    <div class={`forge-card ${props.class ?? ""}`} data-selected={props.selected ? "" : undefined}>
      {props.children}
    </div>
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
