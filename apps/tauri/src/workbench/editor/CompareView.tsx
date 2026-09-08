import { createEffect, onCleanup, onMount } from "solid-js";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { lineNumbers } from "@codemirror/view";
import { unifiedMergeView } from "@codemirror/merge";
import type { ThemeBaseId } from "../../theme/tokens";
import { editorTheme } from "./theme";

/**
 * A6's third answer: what is on disk against what is in the buffer.
 *
 * Read-only and unified. A conflict is a question about two versions of the
 * same file, and the fastest way to answer it is the same view the Diff tab
 * uses for a patch — with the disk copy as the original, so the additions are
 * the edits that have not been written yet.
 */
export function CompareView(props: { disk: string; mine: string; base: ThemeBaseId }) {
  let host!: HTMLDivElement;
  let view: EditorView | undefined;

  function build(): void {
    view?.destroy();
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: props.mine,
        extensions: [
          lineNumbers(),
          EditorView.editable.of(false),
          EditorState.readOnly.of(true),
          editorTheme(props.base),
          unifiedMergeView({ original: props.disk, mergeControls: false }),
        ],
      }),
    });
  }

  onMount(build);
  // Rebuilt rather than reconfigured: `unifiedMergeView` takes its original
  // document as a construction argument, and there is no effect that swaps it.
  createEffect(() => {
    props.disk;
    props.mine;
    props.base;
    if (view) build();
  });
  onCleanup(() => view?.destroy());

  return <div ref={host} class="editor-compare" />;
}
