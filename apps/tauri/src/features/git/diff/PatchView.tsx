import { createMemo, onCleanup } from "solid-js";
import type { PatchRow } from "./patch";
import { intraLine, pairedRows, patchDocument, rowClass } from "./patchDocument";
import { WindowedPatchRows } from "./WindowedPatchRows";
import { nextHunk } from "./patchWindow";
import { scrollPatchRow } from "./patchViewport";
import { cachedPatchSyntax } from "./patchSyntax";
import { PatchText, patchSyntaxColors } from "./PatchText";

export function PatchView(props: {
  path: string;
  patch: string;
  onOpenLine: (line: number) => void;
}) {
  let host!: HTMLDivElement;

  const doc = createMemo(() => patchDocument(props.patch));
  const tokens = cachedPatchSyntax(
    () => props.path,
    () => doc().rows,
  );
  const colors = patchSyntaxColors();

  const intra = createMemo(() => {
    const rows = doc().rows;
    const marks = new Map<number, { from: number; to: number }>();
    for (const pair of pairedRows(rows)) {
      const spans = intraLine(rows[pair.removed]!.text, rows[pair.added]!.text);
      if (!spans) continue;
      if (spans.before.to > spans.before.from) marks.set(pair.removed, spans.before);
      if (spans.after.to > spans.after.from) marks.set(pair.added, spans.after);
    }
    return marks;
  });

  function jumpHunk(forward: boolean): void {
    const target = nextHunk(doc().hunkStarts, Number(host.dataset.cursor ?? "-1"), forward);
    if (target === null) return;
    scrollPatchRow(host, target);
    host.dataset.cursor = String(target);
  }

  function openRow(row: PatchRow): void {
    if (row.after === null) return;
    props.onOpenLine(row.after);
  }

  let pending: "]" | "[" | null = null;
  let pendingTimer = 0;
  onCleanup(() => window.clearTimeout(pendingTimer));

  return (
    <div
      ref={host}
      class="diff-patch"
      style={colors()}
      tabIndex={0}
      onClick={(event) => {
        const row = (event.target as HTMLElement).closest("[data-index]");
        if (row) host.dataset.cursor = row.getAttribute("data-index") ?? "0";
      }}
      onKeyDown={(event) => {
        if (event.key === "]" || event.key === "[") {
          pending = event.key;
          window.clearTimeout(pendingTimer);
          pendingTimer = window.setTimeout(() => {
            pending = null;
          }, 500);
          return;
        }
        if (event.key !== "c" || !pending) return;
        event.preventDefault();
        jumpHunk(pending === "]");
        pending = null;
      }}
    >
      <WindowedPatchRows rows={doc().rows}>
        {(row, index) => (
          <div
            class={`diff-row ${rowClass(row.kind)}`}
            data-index={index()}
            onDblClick={() => openRow(row)}
          >
            <span class="forge-diff-gutter forge-diff-gutter-before">{row.before ?? ""}</span>
            <span class="forge-diff-gutter forge-diff-gutter-after">{row.after ?? ""}</span>
            <span class="forge-diff-gutter forge-diff-gutter-marker">
              {row.kind === "added" ? "+" : row.kind === "removed" ? "−" : ""}
            </span>
            <span class="diff-row-text">
              <PatchText tokens={tokens(row)} mark={intra().get(index())} />
            </span>
          </div>
        )}
      </WindowedPatchRows>
    </div>
  );
}
