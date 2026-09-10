import { For, Show, createMemo } from "solid-js";
import { linkedRefs, openPathRef } from "./pathLinks";
import { splitPathRefs } from "./pathref";

/**
 * A line of output with the files it names made clickable.
 *
 * A button and not an anchor: there is no URL to follow, and a webview handed
 * an `href` navigates away from the app. The text is reproduced verbatim —
 * `path:12` keeps its line number on screen even though the tab opens at it.
 */
export function PathText(props: { text: string }) {
  const pieces = createMemo(() => splitPathRefs(props.text, linkedRefs(props.text)));

  return (
    <For each={pieces()}>
      {(piece) => (
        <Show when={piece.ref} fallback={piece.text}>
          {(ref) => (
            <button
              type="button"
              class="path-link"
              title={`Open ${ref().path}`}
              onClick={() => openPathRef(ref())}
            >
              {piece.text}
            </button>
          )}
        </Show>
      )}
    </For>
  );
}
