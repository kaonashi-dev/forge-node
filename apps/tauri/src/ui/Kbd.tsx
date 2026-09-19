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

/** Renders nothing for unbound actions; `action` takes precedence over `chord`. */
export function Kbd(props: KbdProps) {
  const chord = () => (props.action ? boundChord(props.action) : (props.chord ?? null));
  return (
    <Show when={chord()}>
      {(found) => <kbd class={`forge-kbd ${props.class ?? ""}`}>{describeChord(found())}</kbd>}
    </Show>
  );
}
