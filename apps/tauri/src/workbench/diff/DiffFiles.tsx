import { For, Show, createSignal } from "solid-js";
import { themeBase } from "../../theme/ThemeProvider";
import { LangIcon } from "../../theme/icons";
import { Tooltip } from "../../ui";
import { PatchView } from "./PatchView";
import { SplitPatchView } from "./SplitPatchView";
import type { DiffFile } from "../types";

/**
 * A list of files, each with its patch under a header that stays put (D3).
 *
 * Shared by the Diff tab and the Review tab rather than copied into both: the
 * collapsible section, the budget notes and the double-click-to-open are the
 * same contract in each, and a second copy is a second thing to keep right.
 *
 * Each patch is rendered by CodeMirror and only while its section is open, so
 * a checkout with forty changed files costs the DOM of the ones being read
 * rather than of all of them (§2.3 D1).
 */
export function DiffFiles(props: {
  files: DiffFile[];
  split: boolean;
  onOpenLine: (path: string, line: number) => void;
}) {
  const [collapsed, setCollapsed] = createSignal<Set<string>>(new Set());

  function toggle(path: string): void {
    const next = new Set(collapsed());
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setCollapsed(next);
  }

  return (
    <For each={props.files}>
      {(file) => {
        const open = () => !collapsed().has(file.path);
        return (
          <section class="diff-file">
            <button
              type="button"
              class="forge-row diff-file-head"
              aria-expanded={open()}
              onClick={() => toggle(file.path)}
            >
              <span class="tree-twisty" classList={{ open: open() }}>
                ›
              </span>
              <LangIcon path={file.path} size={13} />
              <span class="git-status" data-status={file.status}>
                {file.status.slice(0, 1)}
              </span>
              <span class="tree-label">{file.path}</span>
              <span class="git-counts">
                <span class="added">+{file.additions}</span>
                <span class="deleted">−{file.deletions}</span>
              </span>
            </button>
            <Show when={open()}>
              {/* A patch over budget is dropped whole and flagged, never cut:
                  half a patch is not a patch. */}
              <Show when={file.truncated}>
                <p class="panel-note">This patch was over budget and is not shown.</p>
              </Show>
              <Show when={file.binary}>
                <p class="panel-note">Binary file.</p>
              </Show>
              <Show when={!file.truncated && !file.binary}>
                <Tooltip label="Double-click a line to open it in the editor" contents>
                  <div class="diff-body">
                    <Show
                      when={props.split}
                      fallback={
                        <PatchView
                          patch={file.patch}
                          base={themeBase()}
                          onOpenLine={(line) => props.onOpenLine(file.path, line)}
                        />
                      }
                    >
                      <SplitPatchView
                        patch={file.patch}
                        base={themeBase()}
                        onOpenLine={(line) => props.onOpenLine(file.path, line)}
                      />
                    </Show>
                  </div>
                </Tooltip>
              </Show>
            </Show>
          </section>
        );
      }}
    </For>
  );
}
