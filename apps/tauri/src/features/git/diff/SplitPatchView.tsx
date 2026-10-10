import { createMemo } from "solid-js";
import { parsePatch, type PatchRow } from "./patch";
import { splitRows } from "./patchDocument";
import { WindowedPatchRows } from "./WindowedPatchRows";
import { cachedPatchSyntax } from "./patchSyntax";
import { PatchText, patchSyntaxColors } from "./PatchText";

export function SplitPatchView(props: {
  path: string;
  patch: string;
  onOpenLine: (line: number) => void;
}) {
  const rows = createMemo(() => parsePatch(props.patch));
  const pairs = createMemo(() => splitRows(rows()));
  const tokens = cachedPatchSyntax(() => props.path, rows);
  const colors = patchSyntaxColors();

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
    <div class="diff-split" style={colors()}>
      <WindowedPatchRows rows={pairs()}>
        {(pair) => (
          <div class="diff-split-row">
            <div
              class={`diff-row ${sideClass(pair.left, "left")}`}
              onDblClick={() => openRow(pair.left)}
            >
              <span class="forge-diff-gutter">{pair.left?.before ?? ""}</span>
              <span class="diff-row-text">
                {pair.left && <PatchText tokens={tokens(pair.left, "before")} />}
              </span>
            </div>
            <div
              class={`diff-row ${sideClass(pair.right, "right")}`}
              onDblClick={() => openRow(pair.right)}
            >
              <span class="forge-diff-gutter">{pair.right?.after ?? ""}</span>
              <span class="diff-row-text">
                {pair.right && <PatchText tokens={tokens(pair.right, "after")} />}
              </span>
            </div>
          </div>
        )}
      </WindowedPatchRows>
    </div>
  );
}
