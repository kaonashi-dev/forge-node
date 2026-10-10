import { patchWindow, type PatchWindow } from "./patchWindow";

type Patch = {
  host: HTMLElement;
  rowCount: () => number;
  publish: (window: PatchWindow) => void;
  lineHeight: number;
};

type Viewport = {
  add: (patch: Patch) => void;
  remove: (patch: Patch) => void;
  refresh: () => void;
};

const viewports = new WeakMap<HTMLElement, Viewport>();

function scrollRoot(host: HTMLElement): HTMLElement {
  // `.diff-body` has horizontal overflow but does not own vertical scrolling.
  return host.closest<HTMLElement>(".compare-diff, .center-view") ?? document.documentElement;
}

function lineHeight(host: HTMLElement): number {
  return Math.max(1, Number.parseFloat(getComputedStyle(host).lineHeight) || 1);
}

function createViewport(root: HTMLElement): Viewport {
  const patches = new Map<HTMLElement, Patch>();
  const active = new Set<Patch>();
  let frame = 0;
  let live = true;

  function paint(): void {
    frame = 0;
    const top = root.getBoundingClientRect().top + root.clientTop;
    const height = root.clientHeight;
    // Read every active block before publishing any DOM changes.
    const updates = Array.from(active, (patch) => ({
      patch,
      window: patchWindow(
        top - patch.host.getBoundingClientRect().top,
        height,
        patch.lineHeight,
        patch.rowCount(),
      ),
    }));
    for (const { patch, window } of updates) patch.publish(window);
  }

  function refresh(): void {
    if (live && active.size > 0 && !frame) frame = requestAnimationFrame(paint);
  }

  const visibility = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const patch = patches.get(entry.target as HTMLElement);
        if (!patch) continue;
        if (entry.isIntersecting) active.add(patch);
        else {
          active.delete(patch);
          patch.publish({ first: 0, end: 0 });
        }
      }
      refresh();
    },
    { root, rootMargin: "512px 0px" },
  );
  const resize = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const patch = patches.get(entry.target as HTMLElement);
      if (patch) patch.lineHeight = lineHeight(patch.host);
    }
    refresh();
  });
  root.addEventListener("scroll", refresh, { passive: true });
  resize.observe(root);
  // Expanding a summary moves patches without resizing the viewport.
  if (root.firstElementChild) resize.observe(root.firstElementChild);

  return {
    refresh,
    add(patch) {
      patches.set(patch.host, patch);
      active.add(patch);
      visibility.observe(patch.host);
      resize.observe(patch.host);
      refresh();
    },
    remove(patch) {
      patches.delete(patch.host);
      active.delete(patch);
      visibility.unobserve(patch.host);
      resize.unobserve(patch.host);
      if (patches.size > 0) {
        refresh();
        return;
      }
      live = false;
      if (frame) cancelAnimationFrame(frame);
      visibility.disconnect();
      resize.disconnect();
      root.removeEventListener("scroll", refresh);
      viewports.delete(root);
    },
  };
}

export function observePatchViewport(
  host: HTMLElement,
  rowCount: () => number,
  publish: (window: PatchWindow) => void,
): { refresh: () => void; dispose: () => void } {
  const root = scrollRoot(host);
  let viewport = viewports.get(root);
  if (!viewport) {
    viewport = createViewport(root);
    viewports.set(root, viewport);
  }
  const patch = { host, rowCount, publish, lineHeight: lineHeight(host) };
  viewport.add(patch);
  let live = true;
  return {
    refresh: viewport.refresh,
    dispose() {
      if (!live) return;
      live = false;
      viewport.remove(patch);
    },
  };
}

export function scrollPatchRow(host: HTMLElement, index: number): void {
  const root = scrollRoot(host);
  const header = host.closest(".diff-file")?.querySelector<HTMLElement>(".diff-file-head");
  root.scrollTop +=
    host.getBoundingClientRect().top -
    root.getBoundingClientRect().top -
    root.clientTop +
    index * lineHeight(host) -
    (header?.offsetHeight ?? 0);
  viewports.get(root)?.refresh();
}
