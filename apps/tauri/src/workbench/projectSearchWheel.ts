/** Vertical-dominant wheel over a long-hit excerpt belongs to `.center-view`. */
export function searchWheelChainsVertically(event: {
  ctrlKey: boolean;
  metaKey: boolean;
  deltaX: number;
  deltaY: number;
}): boolean {
  return !event.ctrlKey && !event.metaKey && Math.abs(event.deltaY) > Math.abs(event.deltaX);
}

type ClosestNode = {
  closest?: (selector: string) => unknown;
  parentElement?: ClosestNode | null;
  parentNode?: ClosestNode | null;
};

/** Wheel over the line's text hits a Text node, which has no `closest`. */
export function isSearchExcerptTarget(target: unknown): boolean {
  let node: ClosestNode | null | undefined =
    target && typeof target === "object" ? (target as ClosestNode) : null;
  while (node && typeof node.closest !== "function") {
    node = node.parentElement ?? node.parentNode;
  }
  return node?.closest?.(".project-search-excerpt") != null;
}
