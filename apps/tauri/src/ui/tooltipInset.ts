export function tooltipOverflowPaddingFor(
  platform: string | undefined,
  trafficLightWidth: string,
): number {
  if (platform !== "macos") return 8;
  const px = Number.parseFloat(trafficLightWidth);
  return Number.isFinite(px) && px > 0 ? px + 8 : 86;
}

/** Viewport padding passed to Kobalte's popper so tips clear macOS traffic lights. */
export function tooltipOverflowPadding(): number {
  if (typeof document === "undefined") return 8;
  return tooltipOverflowPaddingFor(
    document.documentElement.dataset.platform,
    getComputedStyle(document.documentElement).getPropertyValue("--forge-traffic-light-w"),
  );
}

export type TooltipAnchorChild = {
  getBoundingClientRect(): { x: number; y: number; width: number; height: number };
};

export type TooltipAnchor = TooltipAnchorChild & {
  firstElementChild?: TooltipAnchorChild | null;
};

/** A `display: contents` trigger draws no box, so Popper must anchor to its child. */
export function tooltipAnchorRect(anchor?: TooltipAnchor | null) {
  const child = anchor?.firstElementChild;
  if (child) return child.getBoundingClientRect();
  return anchor?.getBoundingClientRect();
}
