import { Show, type JSX } from "solid-js";

/**
 * The shape every settings section is built from.
 *
 * One page header, then groups, then rows. Pulled out so the sections read as
 * content rather than as layout, and so the five of them cannot drift into
 * five slightly different headings.
 */

export type PageProps = {
  title: string;
  /** One line on what the section is for. Not a paragraph. */
  summary?: string;
  children: JSX.Element;
};

export function Page(props: PageProps) {
  return (
    <>
      <header class="settings-page-head">
        <h2 class="settings-page-title">{props.title}</h2>
        <Show when={props.summary}>{(text) => <p class="settings-page-summary">{text()}</p>}</Show>
      </header>
      <div class="settings-groups">{props.children}</div>
    </>
  );
}

export type GroupProps = {
  /** Omit for an unlabelled card. */
  title?: string;
  description?: string;
  /** Sits at the far end of the title line — a count, a refresh. */
  aside?: JSX.Element;
  children: JSX.Element;
  class?: string;
};

/** A bordered card holding one group of related settings. */
export function Group(props: GroupProps) {
  return (
    <section class={`settings-group ${props.class ?? ""}`}>
      <Show when={props.title || props.aside}>
        <div class="settings-group-head">
          <Show when={props.title}>
            {(title) => <h3 class="settings-group-title">{title()}</h3>}
          </Show>
          <Show when={props.aside}>
            {(aside) => <div class="settings-group-aside">{aside()}</div>}
          </Show>
        </div>
      </Show>
      <Show when={props.description}>
        {(text) => <p class="settings-group-description">{text()}</p>}
      </Show>
      {props.children}
    </section>
  );
}

export type RowProps = {
  label: string;
  description?: string;
  /** The control. Goes to the right of the text on a wide window. */
  control?: JSX.Element;
  /** Full-width content under the text, for a field or a list. */
  children?: JSX.Element;
};

/** Label and explanation on the left, the thing you operate on the right. */
export function Row(props: RowProps) {
  return (
    <div class="settings-item">
      <div class="settings-item-head">
        <div class="settings-item-text">
          <span class="settings-item-label">{props.label}</span>
          <Show when={props.description}>
            {(text) => <span class="settings-item-description">{text()}</span>}
          </Show>
        </div>
        <Show when={props.control}>
          {(control) => <div class="settings-item-control">{control()}</div>}
        </Show>
      </div>
      {props.children}
    </div>
  );
}
