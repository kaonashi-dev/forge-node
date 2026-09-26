// A press on a link, told apart from a drag.
//
// The slop matches a cursor click: a twitch must not both open the link and
// leave a selection, and a drag is how the address gets copied.

const SLOP = 4;

export class LinkPress {
  private origin: { x: number; y: number } | null = null;

  get pending(): boolean {
    return this.origin !== null;
  }

  arm(x: number, y: number): void {
    this.origin = { x, y };
  }

  /** True once the pointer has moved enough that this is a drag. Clears the press. */
  drag(x: number, y: number): boolean {
    if (!this.origin) return false;
    if (Math.hypot(x - this.origin.x, y - this.origin.y) <= SLOP) return false;
    this.origin = null;
    return true;
  }

  /** True when the release is still within click distance. Clears the press. */
  release(x: number, y: number): boolean {
    const origin = this.origin;
    this.origin = null;
    return origin !== null && Math.hypot(x - origin.x, y - origin.y) <= SLOP;
  }

  cancel(): void {
    this.origin = null;
  }
}
