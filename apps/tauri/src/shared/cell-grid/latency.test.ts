import { describe, expect, it } from "vitest";
import {
  LatencyProbe,
  LATENCY_SAMPLES,
  MAX_OUTSTANDING,
  PaintProbe,
  RollingWindow,
} from "./latency";

describe("LatencyProbe", () => {
  it("has nothing to report before the first echo", () => {
    expect(new LatencyProbe().p95()).toBeNull();
    expect(new LatencyProbe().percentiles()).toBeNull();
  });

  it("measures from the key to the paint", () => {
    const probe = new LatencyProbe();
    const id = probe.send(1000);
    probe.settle(id, 1012);
    expect(probe.p95()).toBeCloseTo(12);
  });

  // A key that encodes to nothing is never written, so no frame is its echo:
  // dropping it is what keeps the two sides from drifting one apart forever.
  it("drops the keys the host never wrote", () => {
    const probe = new LatencyProbe();
    probe.send(1000);
    const written = probe.send(1005);
    probe.settle(written, 1015);
    expect(probe.percentiles()?.count).toBe(1);
    expect(probe.p95()).toBeCloseTo(10);
  });

  it("ignores a frame that echoed nothing", () => {
    const probe = new LatencyProbe();
    probe.send(1000);
    probe.settle(0, 1010);
    expect(probe.p95()).toBeNull();
  });

  it("keeps a rolling window rather than every sample ever taken", () => {
    const probe = new LatencyProbe();
    for (let index = 0; index < LATENCY_SAMPLES + 40; index += 1) {
      probe.settle(probe.send(0), 5);
    }
    expect(probe.percentiles()?.count).toBe(LATENCY_SAMPLES);
  });

  it("reads the percentiles off the window", () => {
    const probe = new LatencyProbe();
    for (let latency = 1; latency <= 100; latency += 1) {
      probe.settle(probe.send(0), latency);
    }
    const stats = probe.percentiles();
    expect(stats?.p50).toBe(50);
    expect(stats?.p95).toBe(95);
    expect(stats?.p99).toBe(99);
  });

  // The cells editor never echoes, so nothing settles what it sends.
  it("keeps a bounded number of keys waiting for an echo", () => {
    const probe = new LatencyProbe();
    const first = probe.send(0);
    for (let index = 0; index < MAX_OUTSTANDING; index += 1) probe.send(0);
    probe.settle(first, 10);
    expect(probe.p95()).toBeNull();
    const kept = probe.send(0);
    probe.settle(kept, 10);
    expect(probe.p95()).toBe(10);
  });

  it("forgets everything on reset", () => {
    const probe = new LatencyProbe();
    probe.settle(probe.send(0), 10);
    probe.reset();
    expect(probe.p95()).toBeNull();
  });
});

describe("RollingWindow", () => {
  it("reads the same percentiles as sorting the last samples", () => {
    const window = new RollingWindow(16);
    const seen: number[] = [];
    let seed = 7;
    for (let index = 0; index < 200; index += 1) {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      const value = seed % 50;
      window.push(value);
      seen.push(value);
      const last = seen.slice(-16).sort((a, b) => a - b);
      for (const fraction of [0.5, 0.95, 0.99]) {
        const at = Math.min(Math.max(Math.ceil(last.length * fraction) - 1, 0), last.length - 1);
        expect(window.at(fraction)).toBe(last[at]);
      }
    }
  });
});

describe("PaintProbe", () => {
  it("has nothing to say before the first full repaint", () => {
    expect(new PaintProbe().stats()).toBeNull();
  });

  it("reports the percentiles and the worst frame of the window", () => {
    const probe = new PaintProbe();
    for (const ms of [1, 2, 3, 4, 20]) probe.record(ms, 3200);
    const stats = probe.stats();
    expect(stats).not.toBeNull();
    expect(stats?.p50).toBe(3);
    expect(stats?.worst).toBe(20);
    expect(stats?.cells).toBe(3200);
  });

  // The worst frame is what the gate is about, and a rolling window that
  // forgot it would report a budget met by the last hundred easy frames.
  it("keeps the worst frame after it has left the window", () => {
    const probe = new PaintProbe();
    probe.record(30, 3200);
    for (let index = 0; index < LATENCY_SAMPLES + 5; index += 1) probe.record(1, 3200);
    expect(probe.stats()?.worst).toBe(30);
    expect(probe.stats()?.p95).toBe(1);
  });

  it("forgets everything on reset", () => {
    const probe = new PaintProbe();
    probe.record(5, 100);
    probe.reset();
    expect(probe.stats()).toBeNull();
  });
});
