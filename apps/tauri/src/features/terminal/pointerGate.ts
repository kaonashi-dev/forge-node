export interface PointerGate {
  /** `false` for a pane that is mounted but hidden. */
  active: boolean | undefined;
  selectionPending: boolean;
  viewportTerminal: string | null;
  hasRows: boolean;
  connected: boolean;
  /** The terminal this pane is bound to, not the one the window is attached to. */
  boundTerminal: string | null;
}

/**
 * Whether a pane may act on the mouse. The comparison is against the pane's own
 * terminal: the window's attachment stays on the primary pane while a split's
 * second pane shows another session.
 */
export function acceptsPointer(gate: PointerGate): boolean {
  return (
    gate.active !== false &&
    !gate.selectionPending &&
    gate.viewportTerminal !== null &&
    gate.hasRows &&
    gate.connected &&
    gate.boundTerminal === gate.viewportTerminal
  );
}
