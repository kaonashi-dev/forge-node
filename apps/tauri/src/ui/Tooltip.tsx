import { Tooltip as Kobalte } from "@kobalte/core/tooltip";
import type { JSX } from "solid-js";
import { tooltipAnchorRect, tooltipOverflowPadding } from "./tooltipInset";

export type TooltipProps = {
  label: string;
  children: JSX.Element;
  placement?: "top" | "bottom" | "left" | "right";
  /**
   * Take the trigger out of the layout so the child stays a direct flex item.
   *
   * A `display: contents` span draws no box, so the tip anchors to its child.
   */
  contents?: boolean;
};

/**
 * A hover/focus tip.
 *
 * The shell used the `title` attribute for this, which never appears for a
 * keyboard user and cannot be styled. Keep `title` only where the text is a
 * long detail nobody needs to read to operate the control.
 */
export function Tooltip(props: TooltipProps) {
  return (
    <Kobalte
      placement={props.placement ?? "bottom"}
      gutter={6}
      openDelay={400}
      overflowPadding={tooltipOverflowPadding()}
      getAnchorRect={props.contents ? tooltipAnchorRect : undefined}
      slide
      flip
    >
      <Kobalte.Trigger
        as="span"
        class={props.contents ? "forge-tooltip-contents" : "forge-tooltip-trigger"}
      >
        {props.children}
      </Kobalte.Trigger>
      <Kobalte.Portal>
        <Kobalte.Content class="forge-tooltip">{props.label}</Kobalte.Content>
      </Kobalte.Portal>
    </Kobalte>
  );
}
