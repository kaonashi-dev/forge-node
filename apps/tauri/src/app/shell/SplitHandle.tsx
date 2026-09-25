import { createSignal, onCleanup } from "solid-js";
import { clampSplitRatio, SPLIT_RATIO_RANGE } from "../../navigation/centerSplit";

/**
 * A column divider that stores a fraction, not a pixel width.
 *
 * Each side is a terminal or a file, so a remembered pixel width of one
 * column would starve the other after a window resize; the fraction keeps
 * both readable.
 */
export function SplitHandle(props: {
  ratio: number;
  onResize: (ratio: number) => void;
  onCommit: (ratio: number) => void;
  label: string;
}) {
  const [dragging, setDragging] = createSignal(false);
  let startX = 0;
  let startRatio = 0.5;
  let latest = 0.5;
  let parentWidth = 1;

  function onPointerDown(event: PointerEvent): void {
    if (event.button !== 0) return;
    event.preventDefault();
    const host =
      event.currentTarget instanceof HTMLElement ? event.currentTarget.parentElement : null;
    parentWidth = Math.max(1, host?.clientWidth ?? 1);
    event.currentTarget instanceof HTMLElement &&
      event.currentTarget.setPointerCapture(event.pointerId);
    startX = event.clientX;
    startRatio = props.ratio;
    latest = props.ratio;
    setDragging(true);
  }

  function onPointerMove(event: PointerEvent): void {
    if (!dragging()) return;
    latest = clampSplitRatio(startRatio + (event.clientX - startX) / parentWidth);
    props.onResize(latest);
  }

  function end(): void {
    if (!dragging()) return;
    setDragging(false);
    props.onCommit(latest);
  }

  onCleanup(end);

  return (
    <div
      class="resize-handle pane-split-handle"
      classList={{ dragging: dragging() }}
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onDblClick={() => {
        props.onResize(SPLIT_RATIO_RANGE.fallback);
        props.onCommit(SPLIT_RATIO_RANGE.fallback);
      }}
    />
  );
}
