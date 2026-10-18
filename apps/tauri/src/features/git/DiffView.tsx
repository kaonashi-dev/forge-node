import { Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { clearDiffReveal, pendingDiffReveal } from "./diff/reveal";
import { openEditorAt } from "../editor/open";
import { DIFF_SPLIT_KEY, readFlag, writeFlag } from "../../state/preferences";
import { Button, EmptyState, RadioGroup, Skeleton } from "../../ui/index";
import { DiffFiles } from "./diff/DiffFiles";
import { diffTotals, rebaseInProgress, readyToContinue } from "../../contracts/workbench";
import { openFile } from "../files/commands";
import { ConflictView } from "./ConflictView";
import { operationName } from "./gitView";
import { continueReplay } from "./rebaseActions";
import { gitStore } from "./state";
import { loading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";

export function DiffView() {
  const [split, setSplit] = createSignal(readFlag(DIFF_SPLIT_KEY, false));

  const files = createMemo(() => gitStore.diff?.files ?? []);

  // A request for a file the patch does not hold, or left over when the view
  // closes, must not jump a later patch to it.
  createEffect(() => {
    const request = pendingDiffReveal();
    if (request && gitStore.diff && !files().some((file) => file.path === request.path)) {
      clearDiffReveal(request);
    }
  });
  onCleanup(() => {
    const request = pendingDiffReveal();
    if (request) clearDiffReveal(request);
  });

  function setLayout(next: boolean): void {
    setSplit(next);
    writeFlag(DIFF_SPLIT_KEY, next);
  }

  function openAt(path: string, line: number): void {
    const workspace = activeWorkspace();
    if (!workspace) return;
    // The read is started here as well as by the editor's own effect: the tab
    // is about to mount, and one round trip earlier is the difference between
    // landing on the line and landing on an empty buffer that then jumps.
    openEditorAt(path, line);
    void openFile(workspace, path).catch(() => undefined);
  }

  return (
    <div class="diff-view">
      <Show when={gitStore.diffError}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      <Show when={(gitStore.rebase?.conflicts.length ?? 0) > 0}>
        <ConflictView />
      </Show>
      <Show when={rebaseInProgress(gitStore.rebase) && readyToContinue(gitStore.rebase)}>
        <div class="forge-card git-ready">
          <span>Every conflict is staged.</span>
          <span class="history-spacer" />
          <Button variant="primary" size="sm" onClick={continueReplay}>
            Continue {operationName(gitStore.rebase?.operation).toLowerCase()}
          </Button>
        </div>
      </Show>
      <Show
        when={gitStore.diff && gitStore.diff.files.length > 0}
        fallback={
          <Show when={!loading.diff} fallback={<Skeleton label="Reading the diff" rows={6} />}>
            <EmptyState
              message="Nothing uncommitted."
              actions={[{ action: "toggle_git", label: "Open the Git panel", icon: "git-branch" }]}
            />
          </Show>
        }
      >
        <header class="diff-header">
          <span class="diff-header-files">
            {files().length === 1 ? "1 file" : `${files().length} files`}
          </span>
          <span class="git-counts">
            <span class="added">+{diffTotals(gitStore.diff).additions}</span>
            <span class="deleted">−{diffTotals(gitStore.diff).deletions}</span>
          </span>
          <span class="forge-caption">against HEAD</span>
          <span class="history-spacer" />
          <RadioGroup
            label="Patch layout"
            variant="segmented"
            value={split() ? "split" : "unified"}
            onChange={(value) => setLayout(value === "split")}
            options={[
              { value: "unified", label: "Unified" },
              { value: "split", label: "Split" },
            ]}
          />
        </header>
        <DiffFiles
          files={files()}
          split={split()}
          onOpenLine={openAt}
          reveal={pendingDiffReveal()}
          onRevealed={clearDiffReveal}
        />
        <Show when={gitStore.diff?.truncated}>
          <p class="panel-note">Some files were over budget and are not listed.</p>
        </Show>
      </Show>
    </div>
  );
}
