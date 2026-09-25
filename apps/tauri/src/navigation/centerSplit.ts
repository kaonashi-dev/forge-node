import type { CenterMode } from "./viewsStore";
import type { WorkbenchView } from "./views";

/**
 * A two-column split of the centre: a terminal beside a terminal, or a
 * file beside the session that was already on screen.
 *
 * Not a tree of panes. Cmd+D opens one extra column; joining it closes that
 * column. Other Code views (diff, search, a pull request) keep the full
 * width, and the remembered split comes back when a file does.
 */
export type SplitKind = "closed" | "code" | "session";

export type CenterSplitState = {
  kind: SplitKind;
  /**
   * The extra terminal when `kind` is `session`.
   *
   * The original session stays the window's attachment; this is the new
   * shell that occupies the right-hand column.
   */
  extra: string | null;
  /**
   * The terminal the host attached for `extra`, as the host announced it.
   *
   * Not read from the session list: the host reports the split before the
   * daemon's `SessionCreated` reaches the store, and its frames name this id.
   */
  terminal: string | null;
  focused: "primary" | "extra";
};

export const CLOSED_SPLIT: CenterSplitState = {
  kind: "closed",
  extra: null,
  terminal: null,
  focused: "primary",
};

/** Files and editors can sit beside a terminal; a diff or a PR cannot. */
export function viewAllowsTerminalSplit(view: WorkbenchView): boolean {
  return view.kind === "editor-terminal" || view.kind === "preview";
}

/** Whether Cmd+D has something it is allowed to split. */
export function canSplitCenter(mode: CenterMode, view: WorkbenchView): boolean {
  if (mode === "session") return true;
  return viewAllowsTerminalSplit(view);
}

/**
 * What the centre actually paints, given settings, the Code/session mode and
 * the view on the Code strip.
 *
 * A remembered split is not always on screen: Settings, a diff or switching
 * to Code while two terminals are split all hide it without forgetting it.
 */
export function visibleSplit(
  split: CenterSplitState,
  settings: boolean,
  mode: CenterMode,
  view: WorkbenchView,
): SplitKind {
  if (settings || split.kind === "closed") return "closed";
  if (split.kind === "session") return mode === "session" ? "session" : "closed";
  if (mode !== "code") return "closed";
  return viewAllowsTerminalSplit(view) ? "code" : "closed";
}

export const SPLIT_RATIO_RANGE = { min: 0.2, max: 0.8, fallback: 0.5 };

export function clampSplitRatio(value: number): number {
  if (!Number.isFinite(value)) return SPLIT_RATIO_RANGE.fallback;
  return Math.min(Math.max(value, SPLIT_RATIO_RANGE.min), SPLIT_RATIO_RANGE.max);
}
