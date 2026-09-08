import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  on,
  onMount,
  type JSX,
} from "solid-js";

export type ComboboxOption<T> = {
  /** What choosing this row yields. */
  value: T;
  /** Group heading; a heading is emitted when it changes down the list. */
  group?: string;
  /** A disabled row is shown, announced, and not choosable. */
  disabled?: boolean;
  /** The row's contents. The caller owns what a row looks like. */
  render: () => JSX.Element;
};

export type ComboboxProps<T> = {
  /** Already filtered and ordered by the caller. */
  options: ReadonlyArray<ComboboxOption<T>>;
  query: string;
  onQuery: (query: string) => void;
  onChoose: (value: T) => void;
  /**
   * The row the cursor is on, whenever it moves.
   *
   * The branch picker's "Create worktree" button acts on it, so the cursor
   * cannot be private to this component: a footer button that did something
   * other than what `Enter` does would be two controls for one act.
   */
  onCursor?: (value: T | null) => void;
  placeholder: string;
  /** Announced name for the input. */
  label: string;
  /** Sits before the input — the palette's scope chip. */
  leading?: JSX.Element;
  /** Shown in place of the list when there is nothing to show. */
  empty?: JSX.Element;
  /** How many rows the list is tall, as a `--palette-rows` custom property. */
  visibleRows?: number;
  listClass?: string;
  inputClass?: string;
};

/**
 * The filtered list every picker in the shell is (§4.1 U6).
 *
 * The command palette and the branch picker each had their own copy of this:
 * two `onKeyDown` switches, two cursor clamps, two `aria-activedescendant`
 * computations and two `scrollIntoView` effects, already drifting. One
 * component means the keyboard behaves the same in both — and that the file
 * palette gets it for free.
 *
 * **Why this is not Kobalte's `Combobox`.** Kobalte's is a closed, select-like
 * control: it owns a persistent value, filters its own options, and positions
 * its list with a popper in a portal. This is the opposite shape — always
 * open, full-bleed inside a modal, showing every result the caller decided on,
 * with headings, and yielding an *action* rather than a value it keeps. Every
 * one of those is something Kobalte's version would have to be fought out of,
 * so what is shared here is the semantics, hand-held once, rather than a
 * primitive bent into a shape it was not built for. The ARIA below is the
 * combobox pattern in full: `role="combobox"` on the input, `aria-controls`
 * and `aria-activedescendant` onto a `role="listbox"` of `role="option"`.
 */
export function Combobox<T>(props: ComboboxProps<T>) {
  let input!: HTMLInputElement;
  let list: HTMLDivElement | undefined;
  const id = createUniqueId();

  /**
   * The cursor, clamped rather than stored.
   *
   * A stored index outlives the list it indexed: typing one more character can
   * empty the list, and the row at index 4 of nothing is what `Enter` would
   * then choose. Clamping on read means the cursor is always a row that exists.
   */
  const [raw, setRaw] = createSignal(0);
  const cursor = createMemo(() =>
    Math.min(Math.max(raw(), 0), Math.max(props.options.length - 1, 0)),
  );

  // A new query is a new list; the cursor goes back to the top rather than
  // staying on whatever ordinal it held in the previous one.
  createEffect(
    on(
      () => props.query,
      () => setRaw(0),
      { defer: true },
    ),
  );

  function moveTo(index: number): void {
    setRaw(index);
  }

  function move(delta: number): void {
    const count = props.options.length;
    if (count === 0) return;
    // Wraps: a list this short is faster to reach from either end.
    setRaw((((cursor() + delta) % count) + count) % count);
  }

  function choose(option: ComboboxOption<T> | undefined): void {
    if (!option || option.disabled) return;
    props.onChoose(option.value);
  }

  function onKeyDown(event: KeyboardEvent): void {
    // ⌘A / ⌘C belong to the field; the list only answers to unmodified keys.
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        move(-1);
        break;
      case "Home":
        event.preventDefault();
        moveTo(0);
        break;
      case "End":
        event.preventDefault();
        moveTo(props.options.length - 1);
        break;
      case "Enter":
        event.preventDefault();
        choose(props.options[cursor()]);
        break;
      default:
        break;
    }
  }

  // Keep the cursor row on screen without measuring: the list is short, and
  // the browser's own scroll-into-view is exactly the behaviour wanted.
  createEffect(() => {
    const row = list?.querySelector<HTMLElement>(`[data-row="${cursor()}"]`);
    row?.scrollIntoView({ block: "nearest" });
  });

  /** Headings, emitted when the group changes down the filtered list. */
  const headings = createMemo(() => {
    const seen = new Set<string>();
    return props.options.map((option) => {
      if (!option.group || seen.has(option.group)) return null;
      seen.add(option.group);
      return option.group;
    });
  });

  createEffect(() => props.onCursor?.(props.options[cursor()]?.value ?? null));

  const activeId = () => (props.options.length > 0 ? `${id}-row-${cursor()}` : undefined);

  onMount(() => input.focus({ preventScroll: true }));

  return (
    <>
      <div class="palette-query">
        <Show when={props.leading}>{(leading) => leading()}</Show>
        {/* `spellcheck` kills the squiggle; macOS substitution is a separate
            switch, and a query is never prose. */}
        <input
          ref={input}
          class={`forge-field-input palette-input ${props.inputClass ?? ""}`}
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-activedescendant={activeId()}
          aria-label={props.label}
          autocomplete="off"
          spellcheck={false}
          autocorrect="off"
          autocapitalize="off"
          placeholder={props.placeholder}
          value={props.query}
          onInput={(event) => props.onQuery(event.currentTarget.value)}
          onKeyDown={onKeyDown}
        />
      </div>
      <div
        ref={list}
        class={`palette-list ${props.listClass ?? ""}`}
        id={`${id}-list`}
        role="listbox"
        style={props.visibleRows ? { "--palette-rows": String(props.visibleRows) } : undefined}
      >
        <For each={props.options} fallback={props.empty}>
          {(option, index) => (
            <>
              <Show when={headings()[index()]}>
                {(text) => <div class="palette-group">{text()}</div>}
              </Show>
              <div
                class="palette-row"
                id={`${id}-row-${index()}`}
                role="option"
                aria-selected={index() === cursor()}
                aria-disabled={option.disabled}
                data-row={index()}
                classList={{ selected: index() === cursor(), disabled: option.disabled }}
                onMouseEnter={() => moveTo(index())}
                onClick={() => choose(option)}
              >
                {option.render()}
              </div>
            </>
          )}
        </For>
      </div>
    </>
  );
}
