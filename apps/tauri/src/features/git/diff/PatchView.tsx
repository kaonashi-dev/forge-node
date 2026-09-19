import { For, createMemo } from "solid-js";
import type { PatchRow } from "./patch";
import { intraLine, pairedRows, patchDocument, rowClass } from "./patchDocument";

/**
 * One file's patch as a read-only row list.
 *
 * Mounted only while the section is open (`DiffFiles`), so a checkout with
 * many changed files costs the DOM of the ones being read.
 */
export function PatchView(props: { patch: string; onOpenLine: (line: number) => void }) {
  let host!: HTMLDivElement;

  const doc = createMemo(() => patchDocument(props.patch));

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
    const starts = doc().hunkStarts;
    if (starts.length === 0) return;
    const current = Number(host.dataset.cursor ?? "0");
    const target = forward
      ? starts.find((index) => index > current)
      : [...starts].reverse().find((index) => index < current);
    if (target === undefined) return;
    host.querySelector(`[data-index="${target}"]`)?.scrollIntoView({ block: "start" });
    host.dataset.cursor = String(target);
  }

  function openRow(row: PatchRow): void {
    if (row.after === null) return;
    props.onOpenLine(row.after);
  }

  function renderText(index: number, text: string) {
    const span = intra().get(index);
    if (!span) return text;
    return (
      <>
        {text.slice(0, span.from)}
        <mark class="forge-diff-intra">{text.slice(span.from, span.to)}</mark>
        {text.slice(span.to)}
      </>
    );
  }

  let pending: "]" | "[" | null = null;
  let pendingTimer = 0;

  return (
    <div
      ref={host}
      class="diff-patch"
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
      <For each={doc().rows}>
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
            <span class="diff-row-text">{renderText(index(), row.text)}</span>
          </div>
        )}
      </For>
    </div>
  );
}
