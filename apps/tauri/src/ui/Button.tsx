import { Button as Kobalte } from "@kobalte/core/button";
import { Show, splitProps, type JSX } from "solid-js";
import { Tooltip } from "./Tooltip";
import type { ButtonVariant, ControlSize } from "./types";

export type ButtonProps = {
  children: JSX.Element;
  size?: ControlSize;
  variant?: ButtonVariant;
  disabled?: boolean;
  /** A busy control: shows a spinner, blocks the click, keeps its width. */
  loading?: boolean;
  /** A toggle that is currently on — rendered as `aria-pressed`, not a class. */
  selected?: boolean;
  /** A glyph before the label. */
  iconLeading?: JSX.Element;
  /** A glyph after the label — a chevron, an external-link mark. */
  iconTrailing?: JSX.Element;
  class?: string;
  title?: string;
  /** Set when the button opens something that stays open. */
  "aria-expanded"?: boolean;
  "aria-label"?: string;
  onClick?: (event: MouseEvent) => void;
  onMouseDown?: (event: MouseEvent) => void;
};

/**
 * Every command button in the shell.
 *
 * Kobalte's `Button.Root` underneath so that the disabled and focus semantics
 * are the same whether the element ends up being a `<button>` or not; the
 * classes stay the shell's own, because the stylesheet — not the library — is
 * what defines what a Forge button looks like.
 */
export function Button(props: ButtonProps) {
  const [own, rest] = splitProps(props, [
    "children",
    "size",
    "variant",
    "class",
    "loading",
    "selected",
    "iconLeading",
    "iconTrailing",
    "disabled",
  ]);
  const size = () => own.size ?? "md";
  const variant = () => own.variant ?? "ghost";
  return (
    <Kobalte
      class={`forge-control forge-control-${size()} forge-btn forge-btn-${variant()} ${own.class ?? ""}`}
      disabled={own.disabled || own.loading}
      aria-busy={own.loading || undefined}
      aria-pressed={own.selected}
      {...rest}
    >
      <Show
        when={own.loading}
        fallback={
          <Show when={own.iconLeading}>
            {(icon) => <span class="forge-btn-icon">{icon()}</span>}
          </Show>
        }
      >
        <span class="forge-btn-spinner" aria-hidden="true" />
      </Show>
      {own.children}
      <Show when={own.iconTrailing}>{(icon) => <span class="forge-btn-icon">{icon()}</span>}</Show>
    </Kobalte>
  );
}

export type IconButtonProps = Omit<
  ButtonProps,
  "variant" | "aria-label" | "iconLeading" | "iconTrailing"
> & {
  /** Required: an icon alone says nothing to a screen reader. */
  label: string;
  variant?: ButtonVariant;
  /**
   * Drop the tooltip.
   *
   * For the handful of glyphs whose meaning is the thing next to them — the
   * `×` on a notice, a disclosure chevron — where a tip that repeats the label
   * is noise rather than help. The `aria-label` stays either way.
   */
  hideTooltip?: boolean;
  tooltipPlacement?: "top" | "bottom" | "left" | "right";
};

/**
 * A square, chromeless button holding a single glyph.
 *
 * It carries its own tooltip. `title` was what named these before, and a
 * `title` never appears for someone arriving by keyboard — so every icon-only
 * control in the shell was unlabelled for exactly the people who could not
 * guess the glyph. Wrapping here rather than at ~40 call sites is what makes
 * that true of all of them at once (§4.3 U17).
 */
export function IconButton(props: IconButtonProps) {
  const [own, rest] = splitProps(props, [
    "children",
    "loading",
    "label",
    "class",
    "variant",
    "title",
    "hideTooltip",
    "tooltipPlacement",
  ]);
  const button = (
    <Button
      {...rest}
      loading={own.loading}
      variant={own.variant ?? "ghost"}
      class={`forge-icon-button ${own.class ?? ""}`}
      aria-label={own.label}
    >
      <Show when={!own.loading}>{own.children}</Show>
    </Button>
  );
  return (
    <Show when={!own.hideTooltip} fallback={button}>
      <Tooltip label={own.title ?? own.label} placement={own.tooltipPlacement}>
        {button}
      </Tooltip>
    </Show>
  );
}
