import { For } from "solid-js";
import type { HitSegment } from "./fileContentSearch";

/** A line with its search hits marked, the way the editor marks a find. */
export function HitText(props: { segments: HitSegment[] }) {
  return (
    <For each={props.segments}>
      {(segment) => (segment.hit ? <mark class="search-hit">{segment.text}</mark> : segment.text)}
    </For>
  );
}
