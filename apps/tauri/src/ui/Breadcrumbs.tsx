import { For, Show } from "solid-js";

export type Crumb = {
  label: string;
  /** The value handed back on click; the last crumb is not clickable. */
  value: string;
};

export type BreadcrumbsProps = {
  crumbs: ReadonlyArray<Crumb>;
  onChoose: (value: string) => void;
  /** Announced name for the trail — "File path", "Feature". */
  label: string;
  class?: string;
};

/**
 * A path, one clickable segment per level (§5.3).
 *
 * The last crumb is the thing itself and is not a link: clicking where you
 * already are is the one interaction a breadcrumb must not offer. `aria-current`
 * is what says so to a screen reader.
 */
export function Breadcrumbs(props: BreadcrumbsProps) {
  const last = () => props.crumbs.length - 1;
  return (
    <nav class={`forge-breadcrumbs ${props.class ?? ""}`} aria-label={props.label}>
      <ol class="forge-breadcrumb-list">
        <For each={props.crumbs}>
          {(crumb, index) => (
            <li class="forge-breadcrumb-item">
              <Show
                when={index() < last()}
                fallback={
                  <span class="forge-breadcrumb-current" aria-current="page">
                    {crumb.label}
                  </span>
                }
              >
                <button
                  type="button"
                  class="forge-breadcrumb-link"
                  onClick={() => props.onChoose(crumb.value)}
                >
                  {crumb.label}
                </button>
                <span class="forge-breadcrumb-sep" aria-hidden="true">
                  /
                </span>
              </Show>
            </li>
          )}
        </For>
      </ol>
    </nav>
  );
}
