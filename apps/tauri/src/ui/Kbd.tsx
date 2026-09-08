import { Show } from "solid-js";
import { boundChord } from "../actions/bindings";
import { describeChord, type Chord } from "../actions/keys";
import type { ActionId } from "../actions/actions";

export type KbdProps = {
  /** The action whose chord to show. Reads the merged table, never a literal. */
  action?: ActionId;
  /** An explicit chord, for the few places that have one and no action. */
  chord?: Chord | null;
  class?: string;
};

/**
 * One chord, drawn the way the platform writes it (§4.1 U7).
 *
 * `action` and not a string, everywhere it can be. A chord written as a
 * literal in a tooltip is a second copy of the binding table that nothing
 * updates: it survives a rebind, it survives the platform difference between
 * `⌘` and `Ctrl`, and it goes on saying the wrong thing forever. Reading the
 * merged table means a shortcut shown is a shortcut that works.
 *
 * Renders nothing when the action has no chord, so a call site can put one of
 * these beside a label without checking first.
 */
export function Kbd(props: KbdProps) {
  const chord = () => (props.action ? boundChord(props.action) : (props.chord ?? null));
  return (
    <Show when={chord()}>
      {(found) => <kbd class={`forge-kbd ${props.class ?? ""}`}>{describeChord(found())}</kbd>}
    </Show>
  );
}
