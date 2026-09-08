import { Dynamic } from "solid-js/web";
import { ICONS, type ForgeIconName } from "./forgeIcons";

export type IconEmphasis = "full" | "dim";

type IconProps = {
  name: ForgeIconName;
  size?: number;
  class?: string;
  /** CSS color for tintable icons; defaults to the inherited `currentColor`. */
  color?: string;
  emphasis?: IconEmphasis;
  title?: string;
};

/**
 * A lucide glyph, tinted by `currentColor`.
 *
 * Stroke is pinned to 1.5: lucide ships at 2, which at 14px on a 26px row is a
 * heavier line than the chrome around it. Sizes stay on the 12/14/16 rung the
 * rest of the shell uses. The `.forge-icon-*` colour classes set `color`, which
 * the stroke inherits, so nothing here needs to know the palette.
 */
export function Icon(props: IconProps) {
  return (
    <Dynamic
      component={ICONS[props.name]}
      size={props.size ?? 16}
      strokeWidth={1.5}
      class={`forge-icon ${props.class ?? ""}`}
      color={props.color}
      role={props.title ? "img" : undefined}
      aria-hidden={props.title ? undefined : true}
      aria-label={props.title}
      style={props.emphasis === "dim" ? { opacity: 0.55 } : undefined}
    />
  );
}
