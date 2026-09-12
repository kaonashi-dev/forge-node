// Images a Markdown preview names: where each one comes from, and the reads
// that fetch the ones inside the checkout. The WebView never opens a file
// (ADR-012), so a relative `src` becomes a `ReadImage` through the daemon and
// arrives as a `data:` URL. Nothing here imports Solid or Tauri.

import type { ImageContents } from "./types";

export type ImageSource = { kind: "url"; url: string } | { kind: "workspace"; path: string };

/**
 * Reads outstanding at once.
 *
 * The host's workbench queue holds 64 commands and drops the overflow, and a
 * diff or a save has to fit behind a preview full of screenshots.
 */
export const MAX_IMAGE_READS = 4;

/** How long an answer is waited for: a command the host dropped never gets one. */
export const IMAGE_READ_TIMEOUT_MS = 20_000;

const REMOTE = /^https?:\/\/\S+$/i;
const INLINE = /^data:image\/(?:png|jpe?g|gif|webp|avif|bmp|svg\+xml);base64,[a-z0-9+/=\s]+$/i;
const SCHEME = /^[a-z][a-z0-9+.-]*:/i;

/**
 * Where the image `src` in the document at `documentPath` is drawn from, or
 * `null` when it cannot be shown.
 *
 * A relative path is resolved against the document's directory and a leading
 * `/` against the checkout root, the way a forge host reads a repository.
 * Every other scheme — `file:`, `javascript:`, a protocol-relative `//host` —
 * is refused, and so is a path that climbs out of the checkout.
 */
export function imageSource(src: string, documentPath: string): ImageSource | null {
  const raw = src.trim();
  if (raw === "") return null;
  if (REMOTE.test(raw)) return { kind: "url", url: raw };
  if (INLINE.test(raw)) return { kind: "url", url: raw.replace(/\s+/g, "") };
  if (raw.startsWith("//") || SCHEME.test(raw)) return null;

  const bare = raw.replace(/[?#].*$/, "").replace(/\\/g, "/");
  const segments = bare.startsWith("/") ? [] : documentPath.split("/").slice(0, -1);
  for (const part of bare.split("/")) {
    let segment: string;
    try {
      segment = decodeURIComponent(part);
    } catch {
      return null;
    }
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (segments.length === 0) return null;
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  return segments.length === 0 ? null : { kind: "workspace", path: segments.join("/") };
}

export function dataUrl(image: ImageContents): string {
  return `data:${image.mime};base64,${image.data}`;
}

export type ImageReadResult = { url: string } | { error: string };

export type ImageReader = {
  /** A URL for the checkout's image at `path`, once the daemon has read it. */
  read: (workspace: string, path: string) => Promise<string>;
  /** The host's answer for one path. An answer nobody is waiting for is dropped. */
  settle: (workspace: string, path: string, result: ImageReadResult) => void;
};

type Waiter = { resolve: (url: string) => void; reject: (error: Error) => void };

type Read = {
  workspace: string;
  path: string;
  waiters: Waiter[];
  timer: ReturnType<typeof setTimeout> | null;
};

/**
 * Image reads through a command channel whose answers arrive as events.
 *
 * Two asks for the same path while one is out share its answer; at most
 * `inFlight` are sent at a time and the rest wait their turn. Nothing is
 * cached here — how long an image stays good is the caller's question.
 */
export function createImageReader(
  send: (workspace: string, path: string) => Promise<void>,
  limits: { inFlight: number; timeoutMs: number } = {
    inFlight: MAX_IMAGE_READS,
    timeoutMs: IMAGE_READ_TIMEOUT_MS,
  },
): ImageReader {
  const reads = new Map<string, Read>();
  const queued: string[] = [];
  let running = 0;

  const keyOf = (workspace: string, path: string): string => `${workspace}\n${path}`;

  function start(key: string): void {
    const read = reads.get(key);
    if (!read) return;
    running += 1;
    read.timer = setTimeout(
      () => finish(key, { error: "Timed out reading the image." }),
      limits.timeoutMs,
    );
    send(read.workspace, read.path).catch((error: unknown) =>
      finish(key, { error: error instanceof Error ? error.message : String(error) }),
    );
  }

  function pump(): void {
    while (running < limits.inFlight) {
      const key = queued.shift();
      if (key === undefined) return;
      start(key);
    }
  }

  function finish(key: string, result: ImageReadResult): void {
    const read = reads.get(key);
    // Queued reads have not been sent, so nothing can be answering them yet.
    if (!read || read.timer === null) return;
    reads.delete(key);
    clearTimeout(read.timer);
    running -= 1;
    for (const waiter of read.waiters) {
      if ("url" in result) waiter.resolve(result.url);
      else waiter.reject(new Error(result.error));
    }
    pump();
  }

  return {
    read(workspace, path) {
      return new Promise<string>((resolve, reject) => {
        const key = keyOf(workspace, path);
        const existing = reads.get(key);
        if (existing) {
          existing.waiters.push({ resolve, reject });
          return;
        }
        reads.set(key, { workspace, path, waiters: [{ resolve, reject }], timer: null });
        queued.push(key);
        pump();
      });
    },
    settle(workspace, path, result) {
      finish(keyOf(workspace, path), result);
    },
  };
}
