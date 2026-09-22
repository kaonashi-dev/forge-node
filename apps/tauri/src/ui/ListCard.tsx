import { Show, type JSX } from "solid-js";

export type ListCardProps = {
  /** The mark at the start of the header row — a glyph, a state marker. */
  glyph?: JSX.Element;
  /** A short identifier chip (`#42`); it fills in when the card is selected. */
  ident?: string;
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
  /** Draws the card selection: accent edge, outer halo, filled identifier. */
  selected?: boolean;
  class?: string;
};

export function ListCard(props: ListCardProps) {
  return (
    <article
      class={`list-card ${props.class ?? ""}`}
      data-selected={props.selected ? "" : undefined}
    >
      <Show
        when={props.onOpen}
        fallback={
          <header class="list-card-head">
            <Show when={props.glyph}>{(glyph) => glyph()}</Show>
            <Show when={props.ident}>{(ident) => <span class="list-card-id">{ident()}</span>}</Show>
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
            <Show when={props.ident}>{(ident) => <span class="list-card-id">{ident()}</span>}</Show>
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
