// The caret's blink phase.
//
// Presentation, so it lives on this side with the painter rather than in the
// frame the runtime sends — the same reason the cursor and the selection are
// painted here at all (see the note at the top of `renderer.ts`).

/**
 * Half a blink, in milliseconds.
 *
 * `xterm`'s rate, which every emulator since has copied closely enough that a
 * caret at any other speed reads as a different kind of object.
 */
export const BLINK_MS = 530;

/** Whether the reader has asked the system for less movement. */
export function prefersReducedMotion(): boolean {
  return globalThis.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

/**
 * Drives a caret between shown and hidden.
 *
 * Two rules are what separate a caret from a flashing light. It holds steady
 * whenever the phase is not running — an unfocused pane, or a reader who asked
 * for reduced motion — because a blink is an invitation to type and an
 * unfocused pane is not taking any. And typing restarts the phase *shown*:
 * without that, a burst of keys would spend half its frames with the caret
 * hidden under the character about to be placed, which reads as dropped input.
 */
export class CursorBlink {
  private timer: ReturnType<typeof setInterval> | undefined;
  private shown = true;

  /**
   * @param onChange Called only when the phase actually flips, so a caller can
   *   mark the caret's row dirty without filtering out repeats.
   */
  constructor(
    private readonly onChange: (visible: boolean) => void,
    private readonly period = BLINK_MS,
  ) {}

  /** Whether the caret is in its shown half. Steady while parked. */
  get visible(): boolean {
    return this.shown;
  }

  /** Run or park the phase. Parking leaves the caret shown, never hidden. */
  run(enabled: boolean): void {
    if (enabled === (this.timer !== undefined)) return;
    if (!enabled) {
      this.clear();
      this.set(true);
      return;
    }
    this.set(true);
    this.timer = setInterval(() => this.set(!this.shown), this.period);
  }

  /** Activity at the caret: show it and start the interval over. */
  wake(): void {
    if (this.timer === undefined) {
      this.set(true);
      return;
    }
    this.clear();
    this.set(true);
    this.timer = setInterval(() => this.set(!this.shown), this.period);
  }

  /** Drop the interval. The caret is left shown for whatever paints next. */
  dispose(): void {
    this.clear();
    this.set(true);
  }

  private clear(): void {
    if (this.timer !== undefined) clearInterval(this.timer);
    this.timer = undefined;
  }

  private set(next: boolean): void {
    if (this.shown === next) return;
    this.shown = next;
    this.onChange(next);
  }
}
