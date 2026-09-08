import { Badge as Kobalte } from "@kobalte/core/badge";
import type { JSX } from "solid-js";

export type BadgeProps = {
  children: JSX.Element;
  /** Read out in place of the text, when the text alone is a bare number. */
  label?: string;
  tone?: "neutral" | "good" | "warn" | "bad";
  class?: string;
};

/** A small count or status pill. */
export function Badge(props: BadgeProps) {
  return (
    <Kobalte
      class={`forge-badge ${props.class ?? ""}`}
      data-tone={props.tone ?? "neutral"}
      textValue={props.label}
    >
      {props.children}
    </Kobalte>
  );
}
