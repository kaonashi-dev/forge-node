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
 * A Forge glyph, tinted by `currentColor`.
 *
 * Stroke is pinned to 1.5, the weight the set is drawn at on its 16px grid;
 * sizes stay on the 12/14/16 rung the rest of the shell uses. The
 * `.forge-icon-*` colour classes set `color`, which the stroke inherits, so
 * nothing here needs to know the palette: default/`muted` are the neutral
 * marks most rows use, and the tone classes carry meaning where it exists.
 */
export function Icon(props: IconProps) {
  return (
    <Dynamic
      component={ICONS[props.name]}
      size={props.size ?? 16}
      strokeWidth={1.5}
      class={`forge-icon ${props.emphasis === "dim" ? "forge-icon-dim" : ""} ${props.class ?? ""}`}
      color={props.color}
      role={props.title ? "img" : undefined}
      aria-hidden={props.title ? undefined : true}
      aria-label={props.title}
    />
  );
}
