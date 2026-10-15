import { Badge as Kobalte } from "@kobalte/core/badge";
import type { JSX } from "solid-js";

/** `attention` is ember: only for sessions waiting on the user and conflicts. */
export type BadgeTone = "neutral" | "accent" | "good" | "warn" | "bad" | "attention";

export type BadgeProps = {
  children: JSX.Element;
  /** Read out in place of the text, when the text alone is a bare number. */
  label?: string;
  tone?: BadgeTone;
  class?: string;
  /** For a count the surrounding control already names, so it is not read twice. */
  "aria-hidden"?: boolean;
};

/** A small count or status label; the text carries the state, the tone repeats it. */
export function Badge(props: BadgeProps) {
  return (
    <Kobalte
      class={`forge-badge ${props.class ?? ""}`}
      data-tone={props.tone ?? "neutral"}
      textValue={props.label}
      aria-hidden={props["aria-hidden"]}
    >
      {props.children}
    </Kobalte>
  );
}
