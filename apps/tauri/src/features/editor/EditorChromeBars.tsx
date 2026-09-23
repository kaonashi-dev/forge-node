import { For, Show, type JSX } from "solid-js";
import { pathCrumbs, type EditorChrome } from "./editorChrome";

/** The path as segments; `children` sit at the trailing end of the bar. */
export function EditorBreadcrumbs(props: { path: string; children?: JSX.Element }) {
  const crumbs = () => pathCrumbs(props.path);
  return (
    <header class="editor-breadcrumb">
      <ol class="editor-crumbs" aria-label="File path" title={props.path}>
        <For each={crumbs().parents}>
          {(part) => (
            <li class="editor-crumb">
              <span>{part}</span>
              <span class="editor-crumb-sep" aria-hidden="true">
                ›
              </span>
            </li>
          )}
        </For>
        <li class="editor-crumb current" aria-current="page">
          <span>{crumbs().name}</span>
        </li>
      </ol>
      {props.children}
    </header>
  );
}

export function EditorStatusBar(props: { chrome: EditorChrome }) {
  return (
    <footer class="editor-status" aria-label="Editor status">
      <Show when={props.chrome.position}>{(position) => <span>{position()}</span>}</Show>
      <For each={props.chrome.details}>{(fact) => <span>{fact}</span>}</For>
      <span class="editor-status-spacer" />
      <span class="editor-status-mark">{props.chrome.mark}</span>
    </footer>
  );
}

export function EditorConflictNote(props: { children: JSX.Element }) {
  return (
    <div class="editor-conflict">
      <span class="editor-conflict-dot" aria-hidden="true" />
      <span class="editor-conflict-text">This file changed on disk while you were editing it.</span>
      {props.children}
    </div>
  );
}
