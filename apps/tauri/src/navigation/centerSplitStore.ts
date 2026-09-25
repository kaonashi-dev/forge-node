import { createSignal } from "solid-js";
import { CENTER_SPLIT_RATIO_KEY, readScale, writeChoice } from "../state/preferences";
import {
  CLOSED_SPLIT,
  SPLIT_RATIO_RANGE,
  clampSplitRatio,
  type CenterSplitState,
} from "./centerSplit";

const [split, setSplit] = createSignal<CenterSplitState>(CLOSED_SPLIT);
const [ratio, setRatioState] = createSignal(SPLIT_RATIO_RANGE.fallback);

export const centerSplit = split;
export const centerSplitRatio = ratio;

export function readSplitRatio(): number {
  return readScale(
    CENTER_SPLIT_RATIO_KEY,
    SPLIT_RATIO_RANGE.min,
    SPLIT_RATIO_RANGE.max,
    SPLIT_RATIO_RANGE.fallback,
  );
}

export function setCenterSplitRatio(value: number, persist = false): void {
  const next = clampSplitRatio(value);
  setRatioState(next);
  if (persist) writeChoice(CENTER_SPLIT_RATIO_KEY, String(next));
}

export function openCodeSplit(): void {
  setSplit({ kind: "code", extra: null, focused: "primary" });
}

export function openSessionSplit(extra: string): void {
  setSplit({ kind: "session", extra, focused: "extra" });
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
