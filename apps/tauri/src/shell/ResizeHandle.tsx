import { createSignal, onCleanup } from "solid-js";

export type ResizeHandleProps = {
  /** Which edge the handle sits on: the rail grows right, the panel left. */
  side: "left" | "right";
  width: number;
  min: number;
  max: number;
  /** The width a double-click restores: the token default, not a drag origin. */
  fallback: number;
  /** Called on every frame of the drag, for a live layout. */
  onResize: (width: number) => void;
  /** Called once when the pointer is released, for the write that persists. */
  onCommit: (width: number) => void;
  label: string;
};

/**
 * A drag handle between two panels.
 *
 * Pointer capture rather than window listeners: a drag that leaves the window
 * still belongs to the handle, and releasing outside still ends it — which a
 * `mouseup` on `window` misses when the button comes up over another app.
 */
export function ResizeHandle(props: ResizeHandleProps) {
  const [dragging, setDragging] = createSignal(false);
  let start = 0;
  let startWidth = 0;
  let latest = 0;

  function onPointerDown(event: PointerEvent): void {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget instanceof HTMLElement &&
      event.currentTarget.setPointerCapture(event.pointerId);
    start = event.clientX;
    startWidth = props.width;
    latest = props.width;
    setDragging(true);
  }

  function onPointerMove(event: PointerEvent): void {
    if (!dragging()) return;
    const delta = props.side === "left" ? event.clientX - start : start - event.clientX;
    latest = Math.min(Math.max(startWidth + delta, props.min), props.max);
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
      class="resize-handle"
      classList={{ dragging: dragging() }}
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onDblClick={() => {
        // Resets to the layout's own default, the way a column divider does
        // everywhere else. It used to reset to `startWidth`, which is set on
        // pointer-down — so the first click of the double-click had already
        // made "the width to go back to" the width being dragged from, and a
        // double-click on a handle that had never been dragged reset to zero.
        props.onResize(props.fallback);
        props.onCommit(props.fallback);
      }}
    />
  );
}
