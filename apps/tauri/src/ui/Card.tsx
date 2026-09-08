import { Separator as Kobalte } from "@kobalte/core/separator";
import type { JSX } from "solid-js";

export type CardProps = {
  children: JSX.Element;
  class?: string;
};

/** A bordered surface. */
export function Card(props: CardProps) {
  return <div class={`forge-card ${props.class ?? ""}`}>{props.children}</div>;
}

export type SeparatorProps = {
  orientation?: "horizontal" | "vertical";
  class?: string;
};

/** A rule between groups. */
export function Separator(props: SeparatorProps) {
  return (
    <Kobalte
      orientation={props.orientation ?? "horizontal"}
      class={`forge-separator ${props.class ?? ""}`}
    />
  );
}
