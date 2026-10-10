import {
  For,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  type Accessor,
  type JSX,
} from "solid-js";
import { observePatchViewport } from "./patchViewport";
import type { PatchWindow } from "./patchWindow";

export function WindowedPatchRows<T>(props: {
  rows: T[];
  children: (row: T, index: Accessor<number>) => JSX.Element;
}) {
  let host!: HTMLDivElement;
  let subscription: ReturnType<typeof observePatchViewport> | undefined;
  const [window, setWindow] = createSignal<PatchWindow>(
    { first: 0, end: 0 },
    {
      equals: (a, b) => a.first === b.first && a.end === b.end,
    },
  );
  const visible = createMemo(() => props.rows.slice(window().first, window().end));

  onMount(() => {
    subscription = observePatchViewport(host, () => props.rows.length, setWindow);
  });
  createEffect(() => {
    props.rows.length;
    subscription?.refresh();
  });
  onCleanup(() => subscription?.dispose());

  return (
    <div
      ref={host}
      class="diff-window"
      style={{ height: `calc(${props.rows.length} * var(--diff-row-height))` }}
    >
      <div
        class="diff-window-rows"
        style={{ top: `calc(${window().first} * var(--diff-row-height))` }}
      >
        <For each={visible()}>
          {(row, offset) => props.children(row, () => window().first + offset())}
        </For>
      </div>
    </div>
  );
}
