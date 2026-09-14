import { For, createMemo } from "solid-js";

type Row =
  | { kind: "equal"; text: string; left: number; right: number }
  | { kind: "removed"; text: string; left: number; right: null }
  | { kind: "added"; text: string; left: null; right: number };

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
          <div class={`diff-row ${row.kind === "equal" ? "" : `forge-diff-${row.kind}`}`}>
            <span class="forge-diff-gutter forge-diff-gutter-before">{row.left ?? ""}</span>
            <span class="forge-diff-gutter forge-diff-gutter-after">{row.right ?? ""}</span>
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

/** Myers O(ND) on lines, capped: conflict buffers are rarely huge. */
function lineDiff(disk: string, mine: string): Row[] {
  const a = disk.split("\n");
  const b = mine.split("\n");
  // Drop a trailing empty segment from a final newline so both sides match.
  if (a.length > 0 && a[a.length - 1] === "") a.pop();
  if (b.length > 0 && b[b.length - 1] === "") b.pop();

  const n = a.length;
  const m = b.length;
  if (n + m > 20_000) {
    // Budget: fall back to a coarse equal/not view rather than O(ND) blow-up.
    return coarse(a, b);
  }

  const max = n + m;
  const offset = max;
  const v = new Int32Array(2 * max + 1);
  v[offset + 1] = 0;
  const trace: Int32Array[] = [];

  outer: for (let d = 0; d <= max; d += 1) {
    const snap = Int32Array.from(v);
    trace.push(snap);
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && v[offset + k - 1]! < v[offset + k + 1]!)) {
        x = v[offset + k + 1]!;
      } else {
        x = v[offset + k - 1]! + 1;
      }
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x += 1;
        y += 1;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) break outer;
    }
  }

  const edits: Array<{ type: "equal" | "added" | "removed"; line: string }> = [];
  let x = n;
  let y = m;
  for (let d = trace.length - 1; d >= 0; d -= 1) {
    const snap = trace[d]!;
    const k = x - y;
    let prevK: number;
    if (k === -d || (k !== d && snap[offset + k - 1]! < snap[offset + k + 1]!)) {
      prevK = k + 1;
    } else {
      prevK = k - 1;
    }
    const prevX = snap[offset + prevK]!;
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      x -= 1;
      y -= 1;
      edits.push({ type: "equal", line: a[x]! });
    }
    if (d === 0) break;
    if (x === prevX) {
      y -= 1;
      edits.push({ type: "added", line: b[y]! });
    } else {
      x -= 1;
      edits.push({ type: "removed", line: a[x]! });
    }
  }
  edits.reverse();

  const rows: Row[] = [];
  let left = 1;
  let right = 1;
  for (const edit of edits) {
    if (edit.type === "equal") {
      rows.push({ kind: "equal", text: edit.line, left: left++, right: right++ });
    } else if (edit.type === "removed") {
      rows.push({ kind: "removed", text: edit.line, left: left++, right: null });
    } else {
      rows.push({ kind: "added", text: edit.line, left: null, right: right++ });
    }
  }
  return rows;
}

function coarse(a: string[], b: string[]): Row[] {
  const rows: Row[] = [];
  const len = Math.max(a.length, b.length);
  for (let i = 0; i < len; i += 1) {
    const left = a[i];
    const right = b[i];
    if (left !== undefined && right !== undefined && left === right) {
      rows.push({ kind: "equal", text: left, left: i + 1, right: i + 1 });
    } else {
      if (left !== undefined) {
        rows.push({ kind: "removed", text: left, left: i + 1, right: null });
      }
      if (right !== undefined) {
        rows.push({ kind: "added", text: right, left: null, right: i + 1 });
      }
    }
  }
  return rows;
}
