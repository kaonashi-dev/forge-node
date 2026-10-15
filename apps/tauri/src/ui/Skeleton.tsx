import { Index } from "solid-js";

export type SkeletonProps = {
  /** How many placeholder rows to draw. */
  rows?: number;
  /** Announced while the real content is on its way. */
  label: string;
  class?: string;
};

/** Match the expected content height to prevent layout shifts when loading completes. */
export function Skeleton(props: SkeletonProps) {
  return (
    <div
      class={`forge-skeleton ${props.class ?? ""}`}
      role="status"
      aria-busy="true"
      aria-label={props.label}
    >
      <Index each={Array.from({ length: props.rows ?? 5 })}>
        {() => <span class="forge-skeleton-row" />}
      </Index>
    </div>
  );
}
