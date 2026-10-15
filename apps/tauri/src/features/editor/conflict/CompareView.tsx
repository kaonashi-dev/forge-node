import { For, createMemo } from "solid-js";

import { lineDiff } from "./lineDiff";

/**
 * Disk against buffer for a conflict — a unified read-only line list.
 *
 * No merge controls: the person keeps one side via the banner, not by editing
 * here.
 */
export function CompareView(props: { disk: string; mine: string }) {
  const rows = createMemo(() => lineDiff(props.disk, props.mine));

  return (
    <div class="editor-compare diff-patch">
      <For each={rows()}>
        {(row) => (
          <div
            class="diff-row"
            classList={{
              "forge-diff-added": row.kind === "added",
              "forge-diff-removed": row.kind === "removed",
            }}
          >
            <span class="forge-diff-gutter">{row.left ?? ""}</span>
            <span class="forge-diff-gutter">{row.right ?? ""}</span>
            <span class="forge-diff-gutter forge-diff-gutter-marker">
              {row.kind === "added" ? "+" : row.kind === "removed" ? "−" : ""}
            </span>
            <span class="diff-row-text">{row.text}</span>
          </div>
        )}
      </For>
    </div>
  );
}
