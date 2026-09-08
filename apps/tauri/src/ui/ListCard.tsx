import { Show, type JSX } from "solid-js";

export type ListCardProps = {
  /** The mark at the start of the header row — a glyph, a state marker. */
  glyph?: JSX.Element;
  title: JSX.Element;
  /** Sits at the far end of the header — a status badge, a count. */
  aside?: JSX.Element;
  /** The second line: metadata, in muted type. */
  meta?: JSX.Element;
  children?: JSX.Element;
  /** The row of controls along the bottom. */
  actions?: JSX.Element;
  /** Makes the whole card activate; renders it as a button-like row. */
  onOpen?: () => void;
  /** Required when `onOpen` is set: what activating the card does. */
  openLabel?: string;
  class?: string;
};

/**
 * The card a list panel repeats (§4.2 U13).
 *
 * `history-card`, `feature-card` and `pr-card` were three copies of the same
 * arrangement — glyph, title, aside, meta, body, actions — and the three had
 * already drifted in padding, gap and which line the metadata sat on. One
 * component means a list of runs and a list of pull requests read as the same
 * kind of thing, which they are.
 *
 * The whole card is clickable when `onOpen` is given, and then the header is a
 * real `<button>`: a `div` with a click handler is not reachable by keyboard,
 * and these cards are the primary way into a run.
 */
export function ListCard(props: ListCardProps) {
  return (
    <article class={`list-card ${props.class ?? ""}`}>
      <Show
        when={props.onOpen}
        fallback={
          <header class="list-card-head">
            <Show when={props.glyph}>{(glyph) => glyph()}</Show>
            <span class="list-card-title">{props.title}</span>
            <Show when={props.aside}>{(aside) => aside()}</Show>
          </header>
        }
      >
        {(open) => (
          <button
            type="button"
            class="forge-row list-card-head list-card-open"
            aria-label={props.openLabel}
            onClick={() => open()()}
          >
            <Show when={props.glyph}>{(glyph) => glyph()}</Show>
            <span class="list-card-title">{props.title}</span>
            <Show when={props.aside}>{(aside) => aside()}</Show>
          </button>
        )}
      </Show>
      <Show when={props.meta}>{(meta) => <div class="list-card-meta">{meta()}</div>}</Show>
      <Show when={props.children}>{(body) => <div class="list-card-body">{body()}</div>}</Show>
      <Show when={props.actions}>
        {(actions) => <div class="list-card-actions">{actions()}</div>}
      </Show>
    </article>
  );
}
