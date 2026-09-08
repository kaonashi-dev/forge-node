import { createSignal } from "solid-js";

/** How often the shared clock moves. */
const TICK_MS = 15_000;

const [now, setNow] = createSignal(Date.now());

/**
 * One clock for everything that paints elapsed time.
 *
 * Module-level rather than per component: the rail, the tab strip and the
 * features panel all ask the same question — how long since this session last
 * said anything — and three timers answering it would wake the window three
 * times as often for one answer. Fifteen seconds is well inside
 * `WORKING_WINDOW_MS`, so a marker turns over within a tick of being due.
 *
 * Never cleared, and it does not need to be: it outlives every view in a
 * single-window app, and a `setInterval` on a signal nobody reads costs one
 * timer callback that assigns a number.
 */
if (typeof window !== "undefined") {
  window.setInterval(() => setNow(Date.now()), TICK_MS);
}

export { now };
