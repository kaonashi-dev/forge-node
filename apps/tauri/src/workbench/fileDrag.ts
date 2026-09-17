// Pointer gestures belong to the stable explorer host; terminal panes register live identities.
import { validatePath, validateRename } from "./pathOperations";

export type FileDragPayload = { workspaceId: string; path: string; isFile: boolean };
export type TerminalTarget = { session: string; terminal: string };
type TerminalRegistration = { identity: () => TerminalTarget | null; focus: () => void };
const terminals = new Map<HTMLElement, TerminalRegistration>();

export function registerFileTerminal(
  element: HTMLElement,
  registration: TerminalRegistration,
): () => void {
  terminals.set(element, registration);
  element.dataset.fileTerminal = "true";
  return () => {
    terminals.delete(element);
    delete element.dataset.fileTerminal;
  };
}

export function shellReference(root: string, path: string): string {
  if (!root.startsWith("/") || root.length > 4096 || validatePath(path))
    throw new Error("This path cannot be inserted as a terminal reference.");
  for (let at = 0; at < root.length; at++) {
    if (root.charCodeAt(at) < 32 || root.charCodeAt(at) === 127)
      throw new Error("This path cannot be inserted as a terminal reference.");
  }
  return `'${`${root.replace(/\/$/, "")}/${path}`.replaceAll("'", "'\\''")}'`;
}

export function moveDestination(
  source: FileDragPayload,
  workspace: string,
  parent: string,
): string | null {
  if (workspace !== source.workspaceId || (parent && validatePath(parent))) return null;
  const name = source.path.slice(source.path.lastIndexOf("/") + 1);
  const destination = parent ? `${parent}/${name}` : name;
  return validateRename(source.path, destination) ? null : destination;
}

type Drop =
  | { kind: "move"; element: HTMLElement; parent: string; to: string }
  | {
      kind: "paste";
      element: HTMLElement;
      target: TerminalTarget;
      registration: TerminalRegistration;
    };
export type FileDragOptions = {
  workspace: () => { id: string; path: string } | null;
  connected: () => boolean;
  editing: () => boolean;
  expand: (path: string) => void;
  reveal: (path: string) => void;
  move: (workspace: string, from: string, to: string) => Promise<unknown>;
  paste: (session: string, terminal: string, text: string) => Promise<void>;
  error: (message: string) => void;
};
const THRESHOLD = 5;
const HOVER_MS = 600;
const EDGE = 28;
const SCROLL_STEP = 12;

export function createFileDrag(host: HTMLElement, options: FileDragOptions) {
  const doc = host.ownerDocument;
  const win = doc.defaultView!;
  let gesture: {
    payload: FileDragPayload;
    pointer: number;
    x: number;
    y: number;
    started: boolean;
    focus: HTMLElement | null;
  } | null = null;
  let x = 0;
  let y = 0;
  let frame = 0;
  let hoverTimer: ReturnType<typeof setTimeout> | undefined;
  let hoverPath: string | null = null;
  let drop: Drop | null = null;
  let badge: HTMLElement | null = null;
  let swallow = false;
  let epoch = 0;
  let disposed = false;

  function clearHover() {
    clearTimeout(hoverTimer);
    hoverTimer = undefined;
    hoverPath = null;
  }
  function clearVisual() {
    drop?.element.classList.remove("file-drop-target");
    drop = null;
    badge?.remove();
    badge = null;
    host.classList.remove("file-dragging");
    clearHover();
    if (frame) win.cancelAnimationFrame(frame);
    frame = 0;
  }
  function suppressClick() {
    swallow = true;
  }
  function finish(restore: boolean) {
    const previous = gesture;
    gesture = null;
    clearVisual();
    if (!previous) return;
    if (previous.started) suppressClick();
    if (host.hasPointerCapture(previous.pointer)) host.releasePointerCapture(previous.pointer);
    if (restore && previous.focus?.isConnected) previous.focus.focus({ preventScroll: true });
  }
  function cancel() {
    epoch++;
    finish(true);
  }
  function hit(): Drop | null {
    if (!gesture || !options.connected()) return null;
    const element = doc.elementFromPoint(x, y);
    const terminal = element?.closest<HTMLElement>("[data-file-terminal]");
    if (terminal) {
      const registration = terminals.get(terminal);
      const target = registration?.identity();
      return target && registration
        ? { kind: "paste", element: terminal, target, registration }
        : null;
    }
    if (!element || !host.contains(element)) return null;
    if (element.closest("input, textarea, button, [contenteditable=true]")) return null;
    const row = element.closest<HTMLElement>("[data-path]");
    if (row && row.dataset.fileDirectory !== "true") return null;
    const parent = row?.dataset.path ?? "";
    const workspace = options.workspace();
    const to = workspace && moveDestination(gesture.payload, workspace.id, parent);
    return to ? { kind: "move", element: row ?? host, parent, to } : null;
  }
  function paint() {
    frame = 0;
    if (!gesture?.started) return;
    drop?.element.classList.remove("file-drop-target");
    drop = hit();
    drop?.element.classList.add("file-drop-target");
    if (!badge) {
      badge = doc.createElement("div");
      badge.className = "file-drag-label";
      badge.setAttribute("role", "status");
      doc.body.append(badge);
    }
    badge.textContent =
      drop?.kind === "paste"
        ? "Insert reference"
        : drop?.kind === "move"
          ? `Move to ${drop.parent || "checkout root"}`
          : "Cannot drop here";
    badge.style.left = `${x + 12}px`;
    badge.style.top = `${y + 12}px`;
    const parent = drop?.kind === "move" ? drop.parent : null;
    if (parent !== hoverPath) {
      clearHover();
      hoverPath = parent;
      if (parent)
        hoverTimer = setTimeout(() => {
          const current = hit();
          if (
            gesture?.started &&
            hoverPath === parent &&
            current?.kind === "move" &&
            current.parent === parent
          ) {
            options.expand(parent);
            schedule();
          }
        }, HOVER_MS);
    }
    const list = host.querySelector<HTMLElement>(".fw-tree");
    if (list) {
      const rect = list.getBoundingClientRect();
      if (x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom) {
        const delta = y < rect.top + EDGE ? -SCROLL_STEP : y > rect.bottom - EDGE ? SCROLL_STEP : 0;
        const before = list.scrollTop;
        list.scrollTop += delta;
        if (list.scrollTop !== before) schedule();
      }
    }
  }
  function schedule() {
    if (!frame) frame = win.requestAnimationFrame(paint);
  }
  function down(event: PointerEvent, payload: FileDragPayload) {
    if (
      event.button !== 0 ||
      event.ctrlKey ||
      !event.isPrimary ||
      options.editing() ||
      !options.connected()
    )
      return;
    cancel();
    swallow = false;
    const workspace = options.workspace();
    if (!workspace || workspace.id !== payload.workspaceId || validatePath(payload.path)) return;
    gesture = {
      payload,
      pointer: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      started: false,
      focus: doc.activeElement instanceof HTMLElement ? doc.activeElement : null,
    };
    // Capture only after the threshold so an ordinary click still targets its row.
  }
  function move(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointer) return;
    x = event.clientX;
    y = event.clientY;
    if (!gesture.started) {
      if (Math.hypot(x - gesture.x, y - gesture.y) < THRESHOLD) return;
      gesture.started = true;
      host.setPointerCapture(gesture.pointer);
      host.classList.add("file-dragging");
    }
    event.preventDefault();
    event.stopImmediatePropagation();
    schedule();
  }
  function report(error: unknown) {
    options.error(error instanceof Error ? error.message : String(error));
  }
  function insert(
    payload: FileDragPayload,
    target: TerminalTarget,
    registration: TerminalRegistration,
  ) {
    const workspace = options.workspace();
    const current = registration.identity();
    if (
      !options.connected() ||
      workspace?.id !== payload.workspaceId ||
      current?.session !== target.session ||
      current.terminal !== target.terminal
    )
      return;
    try {
      const text = shellReference(workspace.path, payload.path);
      const attempt = epoch;
      void options.paste(target.session, target.terminal, text).catch((error) => {
        if (!disposed && attempt === epoch) report(error);
      });
      registration.focus();
    } catch (error) {
      report(error);
    }
  }
  function up(event: PointerEvent) {
    if (!gesture || event.pointerId !== gesture.pointer) return;
    if (!gesture.started) {
      finish(false);
      return;
    }
    event.preventDefault();
    event.stopImmediatePropagation();
    x = event.clientX;
    y = event.clientY;
    const destination = hit();
    const payload = gesture.payload;
    finish(true);
    if (destination?.kind === "paste")
      insert(payload, destination.target, destination.registration);
    else if (destination?.kind === "move") {
      const attempt = epoch;
      void options
        .move(payload.workspaceId, payload.path, destination.to)
        .then(() => {
          if (!disposed && attempt === epoch && options.workspace()?.id === payload.workspaceId)
            options.reveal(destination.to);
        })
        .catch((error) => {
          if (!disposed && attempt === epoch) report(error);
        });
    }
  }
  function pointerCancel(event: PointerEvent) {
    if (gesture?.pointer === event.pointerId) cancel();
  }
  function nativeDrag(event: Event) {
    if (gesture) event.preventDefault();
  }
  function key(event: KeyboardEvent) {
    if (gesture && event.key === "Escape") {
      event.preventDefault();
      event.stopImmediatePropagation();
      cancel();
    }
  }
  function mouse(event: MouseEvent) {
    if (gesture?.started || (swallow && (event.type === "click" || event.type === "mouseup"))) {
      event.preventDefault();
      event.stopImmediatePropagation();
      if (event.type === "click") swallow = false;
    }
  }
  function reference(event: Event) {
    if (!(event instanceof CustomEvent)) return;
    const detail: unknown = event.detail;
    if (
      !detail ||
      typeof detail !== "object" ||
      !("workspaceId" in detail) ||
      !("path" in detail) ||
      typeof detail.workspaceId !== "string" ||
      typeof detail.path !== "string"
    )
      return;
    for (const [element, registration] of terminals) {
      const target = registration.identity();
      if (element.isConnected && target) {
        insert(
          { workspaceId: detail.workspaceId, path: detail.path, isFile: true },
          target,
          registration,
        );
        return;
      }
    }
    options.error("Open a connected terminal to insert a reference.");
  }
  function nextPointer(event: PointerEvent) {
    if (event.isPrimary && !gesture) swallow = false;
  }
  win.addEventListener("pointermove", move, true);
  win.addEventListener("pointerdown", nextPointer, true);
  win.addEventListener("pointerup", up, true);
  win.addEventListener("pointercancel", pointerCancel, true);
  host.addEventListener("lostpointercapture", pointerCancel);
  host.addEventListener("dragstart", nativeDrag);
  win.addEventListener("keydown", key, true);
  win.addEventListener("blur", cancel);
  for (const name of ["mousedown", "mousemove", "mouseup", "click"] as const)
    win.addEventListener(name, mouse, true);
  host.addEventListener("forge:file-reference", reference);
  return {
    down,
    cancel,
    destroy() {
      disposed = true;
      cancel();
      win.removeEventListener("pointerdown", nextPointer, true);
      win.removeEventListener("pointermove", move, true);
      win.removeEventListener("pointerup", up, true);
      win.removeEventListener("pointercancel", pointerCancel, true);
      host.removeEventListener("lostpointercapture", pointerCancel);
      host.removeEventListener("dragstart", nativeDrag);
      win.removeEventListener("keydown", key, true);
      win.removeEventListener("blur", cancel);
      for (const name of ["mousedown", "mousemove", "mouseup", "click"] as const)
        win.removeEventListener(name, mouse, true);
      host.removeEventListener("forge:file-reference", reference);
    },
  };
}
