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
 * A hover/focus tip for a control that has no visible name.
 *
 * Icon-only buttons, sidebar view glyphs and geometric marks need one. A row
 * or tab that already shows its label does not — wrapping it with a path or a
 * restatement just covers the tree. Native `title` is the same rule: keep it
 * for a truncated error or a colour swatch, not for a labeled name.
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
