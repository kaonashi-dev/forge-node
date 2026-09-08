import { For, Show, type JSX } from "solid-js";
import { RadioGroup } from "./RadioGroup";
import { SearchField } from "./TextField";

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
  /** Given the input, so a panel can focus it from a chord. */
  ref?: (element: HTMLInputElement) => void;
  /** Anything that belongs beside the search box — a count, a refresh button. */
  children?: JSX.Element;
};

/**
 * The scope chips and the search box every list panel opens with (§4.2 U12).
 *
 * History, Features and the feature tab each had their own copy of this, and
 * the three had drifted: one cleared its query with a button, one did not; two
 * announced their search box and one did not; the chip rows used the same
 * class name for different spacing. One component means the filter behaves the
 * same in every panel, which is the whole of the value — it is not a big
 * component.
 */
export function FilterHeader(props: FilterHeaderProps) {
  return (
    <div class="filter-header">
      <For each={props.rows ?? []}>
        {(row) => (
          <RadioGroup
            label={row.label}
            class="filter-chips"
            orientation="horizontal"
            itemClass="forge-chip"
            value={row.value}
            onChange={(value) => row.onChange(value)}
            options={row.options.map((option) => {
              const label = option.label ?? option.value.toLowerCase();
              return { value: option.value, label, render: () => label };
            })}
          />
        )}
      </For>
      <div class="filter-search">
        <SearchField
          class="panel-search"
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
