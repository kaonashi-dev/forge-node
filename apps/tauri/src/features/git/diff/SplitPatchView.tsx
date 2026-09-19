import { For, createMemo } from "solid-js";
import { parsePatch, type PatchRow } from "./patch";
import { splitRows } from "./patchDocument";

/**
 * The same patch, one side per column.
 *
 * Two panes kept level by `splitRows` fillers. Scroll is tied by matching
 * `scrollTop` — both sides have the same row count by construction.
 */
export function SplitPatchView(props: { patch: string; onOpenLine: (line: number) => void }) {
  let leftHost!: HTMLDivElement;
  let rightHost!: HTMLDivElement;
  let syncing = false;

  const pairs = createMemo(() => splitRows(parsePatch(props.patch)));

  function syncScroll(source: HTMLDivElement, target: HTMLDivElement): void {
    if (syncing) return;
    syncing = true;
    target.scrollTop = source.scrollTop;
    syncing = false;
  }

  function openRow(row: PatchRow | null): void {
    if (!row || row.after === null) return;
    props.onOpenLine(row.after);
  }

  function sideClass(row: PatchRow | null, side: "left" | "right"): string {
    if (row === null) return "forge-diff-filler";
    const own = side === "left" ? "removed" : "added";
    if (row.kind === own) return `forge-diff-${own}`;
    return `forge-diff-${row.kind}`;
  }

  return (
    <div class="diff-split">
      <div ref={leftHost} class="diff-split-side" onScroll={() => syncScroll(leftHost, rightHost)}>
        <For each={pairs()}>
          {(pair) => (
            <div
              class={`diff-row ${sideClass(pair.left, "left")}`}
              onDblClick={() => openRow(pair.left)}
            >
              <span class="forge-diff-gutter">{pair.left?.before ?? ""}</span>
              <span class="diff-row-text">{pair.left?.text ?? ""}</span>
            </div>
          )}
        </For>
      </div>
      <div ref={rightHost} class="diff-split-side" onScroll={() => syncScroll(rightHost, leftHost)}>
        <For each={pairs()}>
          {(pair) => (
            <div
              class={`diff-row ${sideClass(pair.right, "right")}`}
              onDblClick={() => openRow(pair.right)}
            >
              <span class="forge-diff-gutter">{pair.right?.after ?? ""}</span>
              <span class="diff-row-text">{pair.right?.text ?? ""}</span>
            </div>
          )}
        </For>
      </div>
    </div>
  );
}
