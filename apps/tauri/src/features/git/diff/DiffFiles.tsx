import { For, Show, createSignal } from "solid-js";
import { LangIcon } from "../../../theme/icons/index";
import { PatchView } from "./PatchView";
import { SplitPatchView } from "./SplitPatchView";
import type { DiffFile } from "../../../contracts/workbench";
import { statusLetter, statusWord } from "../gitView";

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
            <header class="forge-row diff-file-head">
              <button
                type="button"
                class="forge-row diff-file-toggle"
                aria-label={`${open() ? "Collapse" : "Expand"} diff for ${file.path}`}
                aria-expanded={open()}
                onClick={() => toggle(file.path)}
              >
                <span class="tree-twisty" classList={{ open: open() }} aria-hidden="true">
                  ›
                </span>
              </button>
              <LangIcon path={file.path} size={13} />
              <span class="git-status" data-status={file.status} title={statusWord(file.status)}>
                {statusLetter(file.status)}
              </span>
              <button
                type="button"
                class="forge-row diff-file-name"
                aria-label={`Open ${file.path} in Code`}
                title={`Open ${file.path} in Code`}
                onClick={() => props.onOpenLine(file.path, 1)}
              >
                <span class="tree-label">{file.path}</span>
              </button>
              <span class="git-counts">
                <span class="added">+{file.additions}</span>
                <span class="deleted">−{file.deletions}</span>
              </span>
            </header>
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
                <div class="diff-body">
                  <Show
                    when={props.split}
                    fallback={
                      <PatchView
                        path={file.path}
                        patch={file.patch}
                        onOpenLine={(line) => props.onOpenLine(file.path, line)}
                      />
                    }
                  >
                    <SplitPatchView
                      path={file.path}
                      patch={file.patch}
                      onOpenLine={(line) => props.onOpenLine(file.path, line)}
                    />
                  </Show>
                </div>
              </Show>
            </Show>
          </section>
        );
      }}
    </For>
  );
}
