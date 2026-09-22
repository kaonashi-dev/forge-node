import { Index, Show, createMemo, type JSX } from "solid-js";
import { RadioGroup } from "./RadioGroup";
import { SearchField, type FieldSize } from "./TextField";

export type FilterScope<T extends string> = {
  /** The stored value. */
  value: T;
  /** What the chip says. Defaults to `value` lower-cased. */
  label?: string;
};

export type FilterRow<T extends string> = {
  /** Announced name for the group — never rendered, so it can be specific. */
  label: string;
  value: T;
  options: ReadonlyArray<FilterScope<T>>;
  onChange: (value: T) => void;
};

export type FilterHeaderProps = {
  /**
   * Zero or more chip rows above the search box.
   *
   * Typed loosely on purpose: a header may carry a scope row and a status row
   * whose value types have nothing to do with each other, and threading two
   * generics through the component to say so buys nothing a call site needs.
   */
  // oxlint-disable-next-line no-explicit-any
  rows?: ReadonlyArray<FilterRow<any>>;
  query: string;
  onQuery: (value: string) => void;
  placeholder: string;
  /** Announced name for the search box. */
  label: string;
  /**
   * The search box's rung. `sm` suits the side panels this started in; a
   * settings page, where every other field is `md`, wants the same `md` —
   * a search box four pixels shorter than the inputs under it reads as an
   * afterthought rather than as the page's filter.
   */
  size?: FieldSize;
  /** Given the input, so a panel can focus it from a chord. */
  ref?: (element: HTMLInputElement) => void;
  /** Anything that belongs beside the search box — a count, a refresh button. */
  children?: JSX.Element;
};

export function FilterHeader(props: FilterHeaderProps) {
  return (
    <div class="filter-header">
      {/* `Index`: a caller's `rows` array reads the selected value, so it is a
          new array on every click. Keyed on identity that rebuilt the whole
          strip each time; keyed on position the one `RadioGroup` stays put and
          takes the new values through its props. */}
      <Index each={props.rows ?? []}>
        {(row) => {
          // Two steps on purpose. The inner memo only notifies when the
          // caller's own array changes identity, so a row object rebuilt
          // around the same options does not re-map — and does not hand
          // `RadioGroup` a fresh list of objects to diff — on every keystroke
          // in the search box beside it.
          const source = createMemo(() => row().options);
          const options = createMemo(() =>
            source().map((option) => {
              const label = option.label ?? option.value.toLowerCase();
              return { value: option.value, label, render: () => label };
            }),
          );
          return (
            <RadioGroup
              label={row().label}
              class="filter-chips"
              orientation="horizontal"
              itemClass="forge-chip"
              value={row().value}
              onChange={(value) => row().onChange(value)}
              options={options()}
            />
          );
        }}
      </Index>
      <div class="filter-search">
        <SearchField
          class="panel-search"
          size={props.size ?? "sm"}
          aria-label={props.label}
          placeholder={props.placeholder}
          value={props.query}
          ref={props.ref}
          onChange={props.onQuery}
          onClear={() => props.onQuery("")}
        />
        <Show when={props.children}>{(extra) => extra()}</Show>
      </div>
    </div>
  );
}
