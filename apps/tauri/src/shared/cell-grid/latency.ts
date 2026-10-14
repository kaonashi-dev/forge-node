// Key-to-render latency.
//
// Measured from the WebView because only this side sees the whole round trip:
// the IPC hop out, the daemon's PTY write and 8 ms frame, the hop back, and the
// paint. The host closes a sample by echoing the id of the newest keystroke a
// frame rendered — an id rather than a count, because a key that encodes to
// nothing is never written and a count would leave the two sides permanently
// one apart with no way to notice.

/** Rolling window behind the p95 readout. */
export const LATENCY_SAMPLES = 120;

export const LATENCY_BUDGET_MS = 50;

/** Keystrokes awaiting an echo; older ones are dropped, never measured. */
export const MAX_OUTSTANDING = 256;

/**
 * The last `capacity` samples, kept sorted as they arrive.
 *
 * A percentile is read on every echoed frame, so the window pays an insertion
 * per sample instead of a copy and a sort per read.
 */
export class RollingWindow {
  private readonly ring: Float64Array;
  private readonly sorted: Float64Array;
  private head = 0;
  private filled = 0;

  constructor(capacity: number) {
    this.ring = new Float64Array(capacity);
    this.sorted = new Float64Array(capacity);
  }

  get count(): number {
    return this.filled;
  }

  push(value: number): void {
    const capacity = this.ring.length;
    if (this.filled === capacity) {
      this.remove(this.ring[this.head]);
    } else {
      this.filled += 1;
    }
    this.ring[this.head] = value;
    this.head = (this.head + 1) % capacity;
    this.insert(value);
  }

  /** The sample at `fraction` of the sorted window; `null` while empty. */
  at(fraction: number): number | null {
    if (this.filled === 0) return null;
    const index = Math.min(Math.max(Math.ceil(this.filled * fraction) - 1, 0), this.filled - 1);
    return this.sorted[index];
  }

  clear(): void {
    this.head = 0;
    this.filled = 0;
  }

  private insert(value: number): void {
    const last = this.filled - 1;
    const index = this.lowerBound(value, last);
    for (let shift = last; shift > index; shift -= 1) this.sorted[shift] = this.sorted[shift - 1];
    this.sorted[index] = value;
  }

  private remove(value: number): void {
    const index = this.lowerBound(value, this.filled);
    for (let shift = index; shift < this.filled - 1; shift += 1) {
      this.sorted[shift] = this.sorted[shift + 1];
    }
  }

  /** First index in `sorted[0..end)` not less than `value`. */
  private lowerBound(value: number, end: number): number {
    let low = 0;
    let high = end;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (this.sorted[middle] < value) low = middle + 1;
      else high = middle;
    }
    return low;
  }
}

export class LatencyProbe {
  private next = 1;
  private outstanding: { id: number; at: number }[] = [];
  private samples = new RollingWindow(LATENCY_SAMPLES);

  /** Register a keystroke and return the id to send with it. */
  send(now = performance.now()): number {
    const id = this.next;
    this.next += 1;
    this.outstanding.push({ id, at: now });
    // A surface whose frames never echo would otherwise keep every keystroke.
    if (this.outstanding.length > MAX_OUTSTANDING) this.outstanding.shift();
    return id;
  }

  /**
   * Close every sample up to `id`, called once the frame has painted.
   *
   * Keys that produced no bytes are dropped rather than measured: the host
   * never wrote them, so no frame will ever be their echo.
   */
  settle(id: number, now = performance.now()): void {
    if (id <= 0) return;
    let index = 0;
    while (index < this.outstanding.length && this.outstanding[index].id <= id) {
      if (this.outstanding[index].id === id) {
        this.samples.push(now - this.outstanding[index].at);
      }
      index += 1;
    }
    if (index > 0) this.outstanding.splice(0, index);
  }

  /** The 95th percentile of the window, or `null` before the first sample. */
  p95(): number | null {
    return this.samples.at(0.95);
  }

  /** Every sample in the window, for the debug overlay. */
  percentiles(): { p50: number; p95: number; p99: number; count: number } | null {
    const p50 = this.samples.at(0.5);
    const p95 = this.samples.at(0.95);
    const p99 = this.samples.at(0.99);
    if (p50 === null || p95 === null || p99 === null) return null;
    return { p50, p95, p99, count: this.samples.count };
  }

  reset(): void {
    this.outstanding = [];
    this.samples.clear();
  }
}

/**
 * Full-repaint budget in milliseconds.
 *
 * A frame is 16 ms and the coalesce floor on the host is the same, so a paint
 * that took longer than half of it would leave nothing for the rest of the
 * pipeline — the IPC hop, the store write, the browser's own compositing.
 */
export const PAINT_BUDGET_MS = 8;

/**
 * How long the canvas spends painting.
 *
 * Separate from `LatencyProbe`, which measures the whole key-to-render round
 * trip: when that number goes over budget this is what says whether the cost
 * is ours or the wire's, and they are the two halves nobody can tell apart
 * from one figure.
 *
 * Only full repaints are recorded. A partial paint touches whatever rows
 * happened to change, so its duration says more about what the program printed
 * than about the renderer; the gate is "a whole viewport, under budget".
 */
export class PaintProbe {
  private samples = new RollingWindow(LATENCY_SAMPLES);
  private worst = 0;
  private cells = 0;

  /** Record one full repaint of `cells` cells taking `duration` ms. */
  record(duration: number, cells: number): void {
    this.samples.push(duration);
    if (duration > this.worst) this.worst = duration;
    this.cells = cells;
  }

  /** `null` before the first full repaint. */
  stats(): { p50: number; p95: number; worst: number; cells: number } | null {
    const p50 = this.samples.at(0.5);
    const p95 = this.samples.at(0.95);
    if (p50 === null || p95 === null) return null;
    return { p50, p95, worst: this.worst, cells: this.cells };
  }

  reset(): void {
    this.samples.clear();
    this.worst = 0;
    this.cells = 0;
  }
}
