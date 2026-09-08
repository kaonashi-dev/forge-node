import type { JSX } from "solid-js";
import type { ForgeIconName } from "../theme/icons";

/** The four control heights the shell uses. Matches `--forge-control-*`. */
export type ControlSize = "xs" | "sm" | "md" | "lg";

/**
 * Button vocabulary, named by intent and now backed by a real hierarchy:
 *
 * - `primary` — the one confirming action, a solid accent fill with the
 *   foreground `on()` picks. There used to be two names for this (`primary`
 *   drew only a border, `filled` did the fill); they are one variant now.
 * - `secondary` — an elevated surface with no border. The ordinary button.
 * - `ghost` — chromeless and transparent, for toolbars and icon buttons. The
 *   old `quiet` was the same thing under a second name and is folded in.
 * - `danger` — a solid red destructive action, foreground `on()` picks.
 * - `danger-ghost` — the quiet destructive: red text, a red wash on hover, for
 *   a destructive choice that sits inline or in a menu rather than as the CTA.
 */
export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "danger-ghost";

/**
 * One entry in a menu, as data.
 *
 * Menus are described rather than assembled so that a call site never has to
 * know about portals, placement or keyboard nav — `Menu` and `ContextMenu`
 * render the same shape.
 */
export type MenuItem =
  | {
      kind: "item";
      label: string;
      detail?: string;
      disabled?: boolean;
      icon?: ForgeIconName;
      /** Overrides `icon` when the mark is not one of the shell's glyphs. */
      glyph?: JSX.Element;
      destructive?: boolean;
      run: () => void;
    }
  | {
      kind: "submenu";
      label: string;
      icon?: ForgeIconName;
      items: MenuItem[];
    }
  | { kind: "rule" }
  | { kind: "heading"; label: string };
