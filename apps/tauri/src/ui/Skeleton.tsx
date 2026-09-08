import { Index } from "solid-js";

export type SkeletonProps = {
  /** How many placeholder rows to draw. */
  rows?: number;
  /** Row height, as a token name — defaults to the list row height. */
  height?: string;
  /** Announced while the real content is on its way. */
  label: string;
  class?: string;
};

/**
 * Placeholder rows for content that is on its way (§5.3).
 *
 * Replaces `Reading…` in the places where the shape of the answer is already
 * known — a file tree, a list of pull requests, a diff. The point is not the
 * shimmer: it is that the panel does not change size when the answer lands,
 * so nothing under the pointer moves at the moment someone reaches for it.
 *
 * `aria-busy` with a label rather than a live region: a screen reader should
 * hear "loading the file tree" once, not a row count that means nothing.
 */
export function Skeleton(props: SkeletonProps) {
  return (
    <div
      class={`forge-skeleton ${props.class ?? ""}`}
      role="status"
      aria-busy="true"
      aria-label={props.label}
    >
      <Index each={Array.from({ length: props.rows ?? 5 })}>
        {() => (
          <span
            class="forge-skeleton-row"
            style={props.height ? { height: props.height } : undefined}
          />
        )}
      </Index>
    </div>
  );
}
