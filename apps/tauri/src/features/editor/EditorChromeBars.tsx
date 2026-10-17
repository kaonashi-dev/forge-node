import { createEffect, onCleanup, type JSX } from "solid-js";
import type { EditorChrome } from "./editorChrome";
import { setEditorFooter } from "./editorFooter";

/** The file's own controls above the text; the path lives in the card's footer. */
export function EditorBreadcrumbs(props: { children?: JSX.Element }) {
  return <header class="editor-breadcrumb">{props.children}</header>;
}

/** Hands the editor's status to the card footer rather than drawing a second bar. */
export function EditorStatusBar(props: { chrome: EditorChrome }) {
  createEffect(() =>
    setEditorFooter({
      position: props.chrome.position,
      details: props.chrome.details,
      mark: props.chrome.mark,
    }),
  );
  onCleanup(() => setEditorFooter(null));
  return null;
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
