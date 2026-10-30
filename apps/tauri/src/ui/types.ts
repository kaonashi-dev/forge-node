import type { JSX } from "solid-js";
import type { ForgeIconName } from "../theme/icons/index";

/** The four control heights the shell uses. Matches `--forge-control-*`. */
export type ControlSize = "xs" | "sm" | "md" | "lg";

/**
 * Button vocabulary, named by intent:
 *
 * - `primary` — the one confirming action: a solid accent fill.
 * - `secondary` — a raised surface with a hairline edge. The ordinary button.
 * - `ghost` — chromeless, for toolbars and icon buttons.
 * - `danger` — a solid red destructive action, for the confirm in a dialog.
 * - `danger-ghost` — a red-tinted destructive action that is not the dialog's
 *   confirm: removing a worktree from a panel, a destructive choice inline.
 * - `attention` — ember. Only for answering a session that is waiting on the
 *   user or resolving a conflict; nothing else may use it.
 * - `attention-soft` — the same ember as a tint, for the standing reminder
 *   that something is waiting rather than the act of answering it.
 */
export type ButtonVariant =
  | "primary"
  | "secondary"
  | "ghost"
  | "danger"
  | "danger-ghost"
  | "attention"
  | "attention-soft";

/** An icon button at the end of a menu item, run instead of the item's own action. */
export type MenuItemButton = {
  label: string;
  icon: ForgeIconName;
  run: () => void;
};

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
      /** A colour class for `icon`, such as `forge-icon-green`, when it carries state. */
      iconClass?: string;
      /** Overrides `icon` when the mark is not one of the shell's glyphs. */
      glyph?: JSX.Element;
      destructive?: boolean;
      /** Menu entries of their own beside the item, after it in arrow-key order. */
      buttons?: readonly MenuItemButton[];
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
