import { createSignal } from "solid-js";
import { CENTER_SPLIT_RATIO_KEY, readScale, writeChoice } from "../state/preferences";
import {
  CLOSED_SPLIT,
  SPLIT_RATIO_RANGE,
  clampSplitRatio,
  type CenterSplitState,
} from "./centerSplit";

const [split, setSplit] = createSignal<CenterSplitState>(CLOSED_SPLIT);

export const centerSplit = split;

export function readSplitRatio(): number {
  return readScale(
    CENTER_SPLIT_RATIO_KEY,
    SPLIT_RATIO_RANGE.min,
    SPLIT_RATIO_RANGE.max,
    SPLIT_RATIO_RANGE.fallback,
  );
}

export function persistSplitRatio(value: number): void {
  writeChoice(CENTER_SPLIT_RATIO_KEY, String(clampSplitRatio(value)));
}

export function openCodeSplit(): void {
  setSplit({ kind: "code", extra: null, terminal: null, focused: "primary" });
}

/** Open the terminal column, or follow its session onto a new terminal after a restart. */
export function openSessionSplit(extra: string, terminal: string): void {
  const current = split();
  if (current.kind === "session" && current.extra === extra) {
    if (current.terminal !== terminal) setSplit({ ...current, terminal });
    return;
  }
  setSplit({ kind: "session", extra, terminal, focused: "extra" });
}

export function focusSplitPane(pane: CenterSplitState["focused"]): void {
  const current = split();
  if (current.kind === "closed" || current.focused === pane) return;
  setSplit({ ...current, focused: pane });
}

export function closeSplit(): void {
  if (split().kind === "closed") return;
  setSplit(CLOSED_SPLIT);
}

/** Forget a session column whose process is gone. */
export function dropSplitSession(session: string): void {
  const current = split();
  if (current.kind === "session" && current.extra === session) setSplit(CLOSED_SPLIT);
}
