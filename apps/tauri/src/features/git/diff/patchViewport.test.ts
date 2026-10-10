import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { observePatchViewport, scrollPatchRow } from "./patchViewport";

class ElementStub {
  clientTop = 0;
  clientHeight = 400;
  scrollTop = 0;
  offsetHeight = 0;
  rowHeight = 20;
  top = 0;
  reads = 0;
  firstElementChild: HTMLElement | null = null;
  listeners = new Map<string, Set<EventListenerOrEventListenerObject>>();

  constructor(readonly root?: ElementStub) {}

  get element(): HTMLElement {
    return this as unknown as HTMLElement;
  }

  closest(): HTMLElement {
    return (this.root ?? this).element;
  }

  querySelector(): null {
    return null;
  }

  getBoundingClientRect(): DOMRect {
    this.reads += 1;
    return { top: this.top - (this.root?.scrollTop ?? 0) } as DOMRect;
  }

  addEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(listener);
  }

  removeEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
    this.listeners.get(type)?.delete(listener);
  }

  scroll(): void {
    for (const listener of this.listeners.get("scroll") ?? []) {
      if (typeof listener === "function") listener(new Event("scroll"));
    }
  }
}

let intersections: IntersectionStub[];
let resizes: ResizeStub[];
let frames: Map<number, FrameRequestCallback>;
let serial: number;
let subscriptions: ReturnType<typeof observePatchViewport>[];

class IntersectionStub {
  targets = new Set<Element>();
  disconnected = false;

  constructor(readonly callback: IntersectionObserverCallback) {
    intersections.push(this);
  }

  observe(target: Element): void {
    this.targets.add(target);
  }

  unobserve(target: Element): void {
    this.targets.delete(target);
  }

  disconnect(): void {
    this.disconnected = true;
  }

  emit(host: ElementStub, isIntersecting: boolean): void {
    this.callback(
      [{ target: host.element, isIntersecting } as unknown as IntersectionObserverEntry],
      this as unknown as IntersectionObserver,
    );
  }
}

class ResizeStub {
  targets = new Set<Element>();
  disconnected = false;

  constructor(readonly callback: ResizeObserverCallback) {
    resizes.push(this);
  }

  observe(target: Element): void {
    this.targets.add(target);
  }

  unobserve(target: Element): void {
    this.targets.delete(target);
  }

  disconnect(): void {
    this.disconnected = true;
  }

  emit(host: ElementStub): void {
    this.callback(
      [{ target: host.element } as unknown as ResizeObserverEntry],
      this as unknown as ResizeObserver,
    );
  }
}

function flush(): void {
  const callbacks = [...frames.values()];
  frames.clear();
  for (const callback of callbacks) callback(0);
}

function observe(host: ElementStub, count: () => number = () => 2000) {
  const publish = vi.fn();
  const subscription = observePatchViewport(host.element, count, publish);
  subscriptions.push(subscription);
  return { ...subscription, publish };
}

beforeEach(() => {
  intersections = [];
  resizes = [];
  frames = new Map();
  serial = 0;
  subscriptions = [];
  vi.stubGlobal("IntersectionObserver", IntersectionStub);
  vi.stubGlobal("ResizeObserver", ResizeStub);
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    const id = ++serial;
    frames.set(id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
  vi.stubGlobal(
    "getComputedStyle",
    vi.fn((element: ElementStub) => ({ lineHeight: `${element.rowHeight}px` })),
  );
});

afterEach(() => {
  for (const subscription of subscriptions) subscription.dispose();
  vi.unstubAllGlobals();
});

describe("patch viewport", () => {
  it("shares observers and coalesces one scroll burst into one frame", () => {
    const root = new ElementStub();
    const first = observe(new ElementStub(root));
    observe(new ElementStub(root));
    expect(root.listeners.get("scroll")?.size).toBe(1);
    expect(intersections).toHaveLength(1);
    expect(resizes).toHaveLength(1);
    flush();
    first.publish.mockClear();
    const styleReads = vi.mocked(getComputedStyle).mock.calls.length;
    root.scrollTop = 2000;
    for (let index = 0; index < 20; index += 1) root.scroll();
    expect(frames.size).toBe(1);
    flush();
    expect(first.publish).toHaveBeenCalledTimes(1);
    expect(first.publish).toHaveBeenCalledWith({ first: 76, end: 144 });
    expect(vi.mocked(getComputedStyle).mock.calls).toHaveLength(styleReads);
    expect(frames.size).toBe(0);
  });

  it("does not measure offscreen blocks during scrolling", () => {
    const root = new ElementStub();
    const host = new ElementStub(root);
    const patch = observe(host);
    flush();
    intersections[0].emit(host, false);
    const reads = host.reads;
    root.scroll();
    flush();
    expect(host.reads).toBe(reads);
    expect(patch.publish).toHaveBeenLastCalledWith({ first: 0, end: 0 });
    intersections[0].emit(host, true);
    flush();
    expect(patch.publish).toHaveBeenLastCalledWith({ first: 0, end: 44 });
  });

  it("recalculates font metrics and clips to a replacement patch", () => {
    const root = new ElementStub();
    const host = new ElementStub(root);
    let count = 2000;
    const patch = observe(host, () => count);
    flush();
    host.rowHeight = 25;
    resizes[0].emit(host);
    flush();
    expect(patch.publish).toHaveBeenLastCalledWith({ first: 0, end: 40 });
    count = 3;
    patch.refresh();
    flush();
    expect(patch.publish).toHaveBeenLastCalledWith({ first: 0, end: 3 });
  });

  it("updates offsets when content above a patch expands without a scroll", () => {
    const root = new ElementStub();
    const content = new ElementStub(root);
    root.firstElementChild = content.element;
    const host = new ElementStub(root);
    const patch = observe(host);
    flush();
    expect(resizes[0].targets.has(content.element)).toBe(true);
    host.top = 300;
    resizes[0].emit(content);
    flush();
    expect(patch.publish).toHaveBeenLastCalledWith({ first: 0, end: 29 });
  });

  it("reads all block positions before publishing DOM changes", () => {
    const root = new ElementStub();
    const firstHost = new ElementStub(root);
    const secondHost = new ElementStub(root);
    secondHost.top = 300;
    const first = observe(firstHost);
    const second = observe(secondHost);
    first.publish.mockImplementation(() => {
      secondHost.top = 1000;
    });
    flush();
    expect(second.publish).toHaveBeenLastCalledWith({ first: 0, end: 29 });
  });

  it("scrolls to an unmounted hunk in document coordinates", () => {
    const root = new ElementStub();
    root.top = 10;
    root.clientTop = 2;
    root.scrollTop = 100;
    const host = new ElementStub(root);
    host.top = 80;
    const patch = observe(host);
    scrollPatchRow(host.element, 1500);
    expect(root.scrollTop).toBe(30_068);
    flush();
    expect(patch.publish).toHaveBeenLastCalledWith({ first: 1476, end: 1544 });
  });

  it("keeps other files live and releases the last subscription completely", () => {
    const root = new ElementStub();
    const firstHost = new ElementStub(root);
    const first = observe(firstHost);
    const second = observe(new ElementStub(root));
    first.dispose();
    expect(intersections[0].targets.has(firstHost.element)).toBe(false);
    expect(intersections[0].disconnected).toBe(false);
    expect(root.listeners.get("scroll")?.size).toBe(1);
    second.dispose();
    expect(intersections[0].disconnected).toBe(true);
    expect(resizes[0].disconnected).toBe(true);
    expect(root.listeners.get("scroll")?.size).toBe(0);
    expect(frames.size).toBe(0);
    intersections[0].emit(firstHost, true);
    resizes[0].emit(firstHost);
    first.refresh();
    expect(frames.size).toBe(0);
    observe(new ElementStub(root));
    expect(intersections).toHaveLength(2);
  });
});
