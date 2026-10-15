import { Show, type JSX } from "solid-js";
import { boundChord } from "../actions/bindings";
import { describeChord, type Chord } from "../actions/keys";
import type { ActionId } from "../actions/actions";

export type KbdProps = {
  /** The action whose chord to show. Reads the merged table, never a literal. */
  action?: ActionId;
  /** An explicit chord, for the few places that have one and no action. */
  chord?: Chord | null;
  /** A key that is not a bindable chord — `↑↓`, `esc` in a footer hint. */
  children?: JSX.Element;
  class?: string;
};

/** Renders nothing for unbound actions; `action` takes precedence over `chord`, `chord` over `children`. */
export function Kbd(props: KbdProps) {
  const chord = () => (props.action ? boundChord(props.action) : (props.chord ?? null));
  return (
    <Show
      when={chord()}
      fallback={
        <Show when={!props.action && props.chord === undefined && props.children}>
          <kbd class={`forge-kbd ${props.class ?? ""}`}>{props.children}</kbd>
        </Show>
      }
    >
      {(found) => <kbd class={`forge-kbd ${props.class ?? ""}`}>{describeChord(found())}</kbd>}
    </Show>
  );
}
