export const PATCH_OVERSCAN = 24;

export type PatchWindow = { first: number; end: number };

// `top` is the viewport's offset within the patch, not the scroller's scrollTop.
export function patchWindow(
  top: number,
  height: number,
  lineHeight: number,
  rowCount: number,
): PatchWindow {
  const unit = Math.max(1, lineHeight);
  if (height <= 0 || rowCount <= 0) return { first: 0, end: 0 };
  const first = Math.min(rowCount, Math.max(0, Math.floor(top / unit) - PATCH_OVERSCAN));
  const end = Math.min(
    rowCount,
    Math.max(first, Math.ceil((top + height) / unit) + PATCH_OVERSCAN),
  );
  return first === end ? { first: 0, end: 0 } : { first, end };
}

export function nextHunk(starts: number[], current: number, forward: boolean): number | null {
  if (forward) return starts.find((index) => index > current) ?? null;
  for (let index = starts.length - 1; index >= 0; index -= 1) {
    if (starts[index] < current) return starts[index];
  }
  return null;
}
