// Key-to-render latency (§2.9).
//
// Measured from the WebView because only this side sees the whole round trip:
// the IPC hop out, the daemon's PTY write and 8 ms frame, the hop back, and the
// paint. The host closes a sample by echoing the id of the newest keystroke a
// frame rendered — an id rather than a count, because a key that encodes to
// nothing is never written and a count would leave the two sides permanently
// one apart with no way to notice.

/** Rolling window behind the p95 readout. */
export const LATENCY_SAMPLES = 120;

/** The gate Phase 2 has to pass. */
export const LATENCY_BUDGET_MS = 50;

export class LatencyProbe {
  private next = 1;
  private outstanding: { id: number; at: number }[] = [];
  private samples: number[] = [];

  /** Register a keystroke and return the id to send with it. */
  send(now = performance.now()): number {
    const id = this.next;
    this.next += 1;
    this.outstanding.push({ id, at: now });
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
        this.record(now - this.outstanding[index].at);
      }
      index += 1;
    }
    this.outstanding.splice(0, index);
  }

  private record(latency: number): void {
    this.samples.push(latency);
    if (this.samples.length > LATENCY_SAMPLES) this.samples.shift();
  }

  /** The 95th percentile of the window, or `null` before the first sample. */
  p95(): number | null {
    if (this.samples.length === 0) return null;
    const sorted = [...this.samples].sort((a, b) => a - b);
    const index = Math.min(Math.max(Math.ceil(sorted.length * 0.95) - 1, 0), sorted.length - 1);
    return sorted[index];
  }

  /** Every sample in the window, for the debug overlay. */
  percentiles(): { p50: number; p95: number; p99: number; count: number } | null {
    if (this.samples.length === 0) return null;
    const sorted = [...this.samples].sort((a, b) => a - b);
    const at = (fraction: number) =>
      sorted[Math.min(Math.max(Math.ceil(sorted.length * fraction) - 1, 0), sorted.length - 1)];
    return { p50: at(0.5), p95: at(0.95), p99: at(0.99), count: sorted.length };
  }

  reset(): void {
    this.outstanding = [];
    this.samples = [];
  }
}

/**
 * The budget one full repaint has, in milliseconds (§6.6).
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
  private samples: number[] = [];
  private worst = 0;
  private cells = 0;

  /** Record one full repaint of `cells` cells taking `duration` ms. */
  record(duration: number, cells: number): void {
    this.samples.push(duration);
    if (this.samples.length > LATENCY_SAMPLES) this.samples.shift();
    if (duration > this.worst) this.worst = duration;
    this.cells = cells;
  }

  /** `null` before the first full repaint. */
  stats(): { p50: number; p95: number; worst: number; cells: number } | null {
    if (this.samples.length === 0) return null;
    const sorted = [...this.samples].sort((left, right) => left - right);
    const at = (fraction: number) =>
      sorted[Math.min(Math.max(Math.ceil(sorted.length * fraction) - 1, 0), sorted.length - 1)];
    return { p50: at(0.5), p95: at(0.95), worst: this.worst, cells: this.cells };
  }

  reset(): void {
    this.samples = [];
    this.worst = 0;
    this.cells = 0;
  }
}
