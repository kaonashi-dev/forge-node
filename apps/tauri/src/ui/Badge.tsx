import { Badge as Kobalte } from "@kobalte/core/badge";
import { Show, type JSX } from "solid-js";

/** `attention` is ember: only for sessions waiting on the user and conflicts. */
export type BadgeTone = "neutral" | "accent" | "good" | "warn" | "bad" | "attention";

export type BadgeProps = {
  children: JSX.Element;
  /** Read out in place of the text, when the text alone is a bare number. */
  label?: string;
  tone?: BadgeTone;
  /**
   * Announce changes as a `role="status"` live region. Off by default: a badge
   * inside a row or a button is part of that control's name, and a live region
   * there is read out of context every time it mounts or changes.
   */
  live?: boolean;
  class?: string;
  /** For a count the surrounding control already names, so it is not read twice. */
  "aria-hidden"?: boolean;
};

/** A small count or status label; the text carries the state, the tone repeats it. */
export function Badge(props: BadgeProps) {
  const className = () => `forge-badge ${props.class ?? ""}`;
  const tone = () => props.tone ?? "neutral";
  return (
    <Show
      when={props.live}
      fallback={
        <span class={className()} data-tone={tone()} aria-hidden={props["aria-hidden"]}>
          <Show when={props.label} fallback={props.children}>
            {(label) => (
              <>
                <span aria-hidden="true">{props.children}</span>
                <span class="forge-visually-hidden">{label()}</span>
              </>
            )}
          </Show>
        </span>
      }
    >
      <Kobalte
        class={className()}
        data-tone={tone()}
        textValue={props.label}
        aria-hidden={props["aria-hidden"]}
      >
        {props.children}
      </Kobalte>
    </Show>
  );
}
