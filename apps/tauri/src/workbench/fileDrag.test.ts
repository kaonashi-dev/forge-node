import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createFileDrag,
  moveDestination,
  registerFileTerminal,
  shellReference,
  type FileDragOptions,
} from "./fileDrag";

class Node extends EventTarget {
  dataset: Record<string, string> = {};
  classList = { add: vi.fn(), remove: vi.fn() };
  style: Record<string, string> = {};
  className = "";
  textContent = "";
  isConnected = true;
  scroll = 0;
  get scrollTop() {
    return this.scroll;
  }
  set scrollTop(value: number) {
    this.scroll = Math.max(0, Math.min(24, value));
  }
  scrollport: Node | null = null;
  getBoundingClientRect() {
    return { left: 0, right: 100, top: 0, bottom: 100 };
  }
  parent: Node | null = null;
  children: Node[] = [];
  captured: number | null = null;
  focus = vi.fn();
  ownerDocument!: Doc;
  setAttribute() {}
  append(node: Node) {
    node.parent = this;
    this.children.push(node);
  }
  remove() {
    this.isConnected = false;
  }
  contains(node: Node): boolean {
    return this === node || this.children.some((child) => child.contains(node));
  }
  closest(selector: string): Node | null {
    if (selector === "[data-file-terminal]" && this.dataset.fileTerminal) return this;
    if (selector === "[data-path]" && this.dataset.path !== undefined) return this;
    return this.parent?.closest(selector) ?? null;
  }
  querySelector(): Node | null {
    return this.scrollport;
  }
  setPointerCapture(id: number) {
    this.captured = id;
  }
  hasPointerCapture(id: number) {
    return this.captured === id;
  }
  releasePointerCapture() {
    this.captured = null;
  }
}
class Win extends EventTarget {
  frames = new Map<number, FrameRequestCallback>();
  next = 0;
  requestAnimationFrame(callback: FrameRequestCallback) {
    this.frames.set(++this.next, callback);
    return this.next;
  }
  cancelAnimationFrame(id: number) {
    this.frames.delete(id);
  }
  paint() {
    const frames = [...this.frames.values()];
    this.frames.clear();
    for (const callback of frames) callback(0);
  }
}
class Doc {
  defaultView = new Win();
  body = new Node();
  activeElement = new Node();
  hit: Node | null = null;
  elementFromPoint() {
    return this.hit;
  }
  createElement() {
    return new Node();
  }
}
function event(type: string, extra: Record<string, unknown> = {}) {
  return Object.assign(new Event(type, { cancelable: true }), {
    pointerId: 1,
    button: 0,
    isPrimary: true,
    ctrlKey: false,
    clientX: 10,
    clientY: 10,
    ...extra,
  });
}
const disposals: (() => void)[] = [];
afterEach(() => {
  for (const dispose of disposals.splice(0)) dispose();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});
function setup() {
  vi.useFakeTimers();
  vi.stubGlobal("HTMLElement", Node);
  const doc = new Doc();
  const host = new Node();
  host.ownerDocument = doc;
  const options: FileDragOptions = {
    workspace: () => ({ id: "w", path: "/checkout" }),
    connected: () => true,
    editing: () => false,
    expand: vi.fn(),
    reveal: vi.fn(),
    move: vi.fn().mockResolvedValue(undefined),
    paste: vi.fn().mockResolvedValue(undefined),
    error: vi.fn(),
  };
  const drag = createFileDrag(host as unknown as HTMLElement, options);
  disposals.push(() => drag.destroy());
  function down() {
    drag.down(event("pointerdown") as PointerEvent, {
      workspaceId: "w",
      path: "src/a '雪'.ts",
      isFile: true,
    });
  }
  function send(type: string, extra = {}) {
    const e = event(type, extra);
    doc.defaultView.dispatchEvent(e);
    return e;
  }
  function start() {
    down();
    send("pointermove", { clientX: 30 });
    doc.defaultView.paint();
  }
  function folder(path: string) {
    const node = new Node();
    node.dataset = { path, fileDirectory: "true" };
    host.append(node);
    doc.hit = node;
    return node;
  }
  function terminal() {
    const node = new Node();
    const identity = vi.fn(() => ({ session: "session-A", terminal: "terminal-A" }));
    const focus = vi.fn();
    disposals.push(registerFileTerminal(node as unknown as HTMLElement, { identity, focus }));
    doc.hit = node;
    return { node, identity, focus };
  }
  return { doc, host, options, drag, down, send, start, folder, terminal };
}

describe("file references", () => {
  it("quotes absolute Unicode paths, apostrophes and shell metacharacters without Enter", () => {
    expect(shellReference("/a b/", "a'雪$(pwd).ts")).toBe("'/a b/a'\\''雪$(pwd).ts'");
    expect(shellReference("/", "a")).toBe("'/a'");
    for (const path of ["../a", "/a", "a\ncommand", "a\u001b"])
      expect(() => shellReference("/root", path)).toThrow();
    expect(() => shellReference("/root\n", "a")).toThrow();
  });
  it("only moves to a different directory in the same checkout", () => {
    const source = { workspaceId: "w", path: "src/a", isFile: false };
    expect(moveDestination(source, "w", "")).toBe("a");
    expect(moveDestination(source, "other", "lib")).toBeNull();
    expect(moveDestination(source, "w", "src")).toBeNull();
    expect(moveDestination(source, "w", "src/a/sub")).toBeNull();
  });
});

describe("pointer drag lifecycle", () => {
  it("does not expand a departed hover when its deadline beats the next paint", () => {
    const s = setup();
    s.folder("first");
    s.start();
    vi.advanceTimersByTime(599);
    s.folder("second");
    s.send("pointermove");
    vi.advanceTimersByTime(1);
    expect(s.options.expand).not.toHaveBeenCalled();
  });
  it("suppresses release after a long cancelled hold without swallowing the next gesture", () => {
    const s = setup();
    s.start();
    s.send("keydown", { key: "Escape" });
    vi.advanceTimersByTime(5000);
    expect(s.send("click").defaultPrevented).toBe(true);
    s.start();
    s.drag.cancel();
    s.send("pointerdown");
    expect(s.send("click").defaultPrevented).toBe(false);
  });
  it("coalesces pointer moves and stops edge scrolling at the scroll limit", () => {
    const s = setup();
    s.host.scrollport = new Node();
    s.doc.hit = s.host;
    s.down();
    for (let i = 0; i < 100; i++) s.send("pointermove", { clientY: 95 });
    expect(s.doc.defaultView.frames.size).toBe(1);
    s.doc.defaultView.paint();
    expect(s.host.scrollport.scrollTop).toBe(12);
    s.doc.defaultView.paint();
    expect(s.host.scrollport.scrollTop).toBe(24);
    s.doc.defaultView.paint();
    expect(s.doc.defaultView.frames.size).toBe(0);
    expect(s.options.move).not.toHaveBeenCalled();
    expect(s.options.paste).not.toHaveBeenCalled();
    s.send("pointerup");
    expect(s.options.move).toHaveBeenCalledWith("w", "src/a '雪'.ts", "a '雪'.ts");
  });
  it("rejects files as destinations and refuses a release after losing the connection", () => {
    const s = setup();
    const row = s.folder("file");
    row.dataset.fileDirectory = "false";
    s.start();
    s.send("pointerup");
    expect(s.options.move).not.toHaveBeenCalled();
    s.terminal();
    s.start();
    s.options.connected = () => false;
    s.send("pointerup");
    expect(s.options.paste).not.toHaveBeenCalled();
  });
  it("leaves a plain row click alone and captures the stable root only after threshold", () => {
    const s = setup();
    s.down();
    s.send("pointermove", { clientX: 12 });
    expect(s.host.captured).toBeNull();
    s.send("pointerup");
    expect(s.send("click").defaultPrevented).toBe(false);
    s.start();
    expect(s.host.captured).toBe(1);
    expect(s.send("mousemove").defaultPrevented).toBe(true);
    s.send("pointerup");
    expect(s.host.captured).toBeNull();
    expect(s.send("mouseup").defaultPrevented).toBe(true);
    expect(s.send("click").defaultPrevented).toBe(true);
    expect(s.send("click").defaultPrevented).toBe(false);
  });
  it("pastes once into the exact terminal under release, never opens the file", () => {
    const s = setup();
    const t = s.terminal();
    s.start();
    expect(s.doc.body.children.at(-1)?.textContent).toBe("Insert reference");
    s.send("pointerup");
    s.send("pointerup");
    expect(s.options.paste).toHaveBeenCalledOnce();
    expect(s.options.paste).toHaveBeenCalledWith(
      "session-A",
      "terminal-A",
      "'/checkout/src/a '\\''雪'\\''.ts'",
    );
    expect(s.options.move).not.toHaveBeenCalled();
    expect(t.focus).toHaveBeenCalledOnce();
  });
  it("moves on release without waiting for a paint and reveals only after confirmation", async () => {
    const s = setup();
    let resolve!: () => void;
    s.options.move = vi.fn(
      () =>
        new Promise<void>((done) => {
          resolve = done;
        }),
    );
    s.start();
    s.folder("lib");
    s.send("pointerup");
    expect(s.options.move).toHaveBeenCalledWith("w", "src/a '雪'.ts", "lib/a '雪'.ts");
    expect(s.options.reveal).not.toHaveBeenCalled();
    resolve();
    await Promise.resolve();
    expect(s.options.reveal).toHaveBeenCalledWith("lib/a '雪'.ts");
  });
  it.each(["Escape", "pointercancel", "unmount", "reconnect", "blur"])(
    "cancels %s without writes, stale hover work or lost focus",
    (reason) => {
      const s = setup();
      s.folder("lib");
      s.start();
      if (reason === "Escape") s.send("keydown", { key: "Escape" });
      else if (reason === "unmount") s.drag.destroy();
      else if (reason === "reconnect") s.drag.cancel();
      else s.send(reason);
      vi.advanceTimersByTime(1000);
      s.doc.defaultView.paint();
      s.send("pointerup");
      expect(s.options.move).not.toHaveBeenCalled();
      expect(s.options.paste).not.toHaveBeenCalled();
      expect(s.options.expand).not.toHaveBeenCalled();
      expect(s.host.captured).toBeNull();
      expect(s.doc.activeElement.focus).toHaveBeenCalled();
    },
  );
  it("bounds hover expansion and replaces its deadline when the destination changes", () => {
    const s = setup();
    s.folder("first");
    s.start();
    vi.advanceTimersByTime(400);
    s.folder("second");
    s.send("pointermove");
    s.doc.defaultView.paint();
    vi.advanceTimersByTime(400);
    expect(s.options.expand).not.toHaveBeenCalled();
    vi.advanceTimersByTime(200);
    expect(s.options.expand).toHaveBeenCalledWith("second");
    vi.advanceTimersByTime(5000);
    expect(s.options.expand).toHaveBeenCalledOnce();
  });
  it("ignores another pointer and a stale move completion after a workspace round trip", async () => {
    const s = setup();
    let resolve!: () => void;
    s.options.move = vi.fn(
      () =>
        new Promise<void>((done) => {
          resolve = done;
        }),
    );
    s.folder("lib");
    s.start();
    s.send("pointerup", { pointerId: 2 });
    expect(s.options.move).not.toHaveBeenCalled();
    s.send("pointerup");
    s.drag.cancel();
    resolve();
    await Promise.resolve();
    expect(s.options.reveal).not.toHaveBeenCalled();
  });
  it("refuses a destination that changes identity during release validation", () => {
    const s = setup();
    const t = s.terminal();
    s.start();
    t.identity.mockReturnValueOnce({ session: "old", terminal: "old" });
    s.send("pointerup");
    expect(s.options.paste).not.toHaveBeenCalled();
  });
  it("consumes the menu event through the same targeted paste path", () => {
    class Custom extends Event {
      detail: unknown;
      constructor(detail: unknown) {
        super("forge:file-reference");
        this.detail = detail;
      }
    }
    vi.stubGlobal("CustomEvent", Custom);
    const s = setup();
    s.terminal();
    s.host.dispatchEvent(new Custom({ workspaceId: "w", path: "menu file" }));
    expect(s.options.paste).toHaveBeenCalledOnce();
    expect(s.options.paste).toHaveBeenCalledWith(
      "session-A",
      "terminal-A",
      "'/checkout/menu file'",
    );
  });
});
