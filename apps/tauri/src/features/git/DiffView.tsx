import { Show, createMemo, createSignal } from "solid-js";
import { openEditorAt } from "../editor/open";
import { DIFF_SPLIT_KEY, readFlag, writeFlag } from "../../state/preferences";
import { Button, EmptyState, Skeleton } from "../../ui/index";
import { DiffFiles } from "./diff/DiffFiles";
import { diffTotals } from "../../contracts/workbench";
import { openFile } from "../files/commands";
import { gitStore } from "./state";
import { loading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";

export function DiffView() {
  const [split, setSplit] = createSignal(readFlag(DIFF_SPLIT_KEY, false));

  const files = createMemo(() => gitStore.diff?.files ?? []);

  function toggleSplit(): void {
    const next = !split();
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
          <span>{gitStore.diff?.branch ?? "detached"}</span>
          <span class="git-counts">
            <span class="added">+{diffTotals(gitStore.diff).additions}</span>
            <span class="deleted">−{diffTotals(gitStore.diff).deletions}</span>
          </span>
          <span class="history-spacer" />
          <Button variant="secondary" size="xs" selected={split()} onClick={toggleSplit}>
            {split() ? "Split" : "Unified"}
          </Button>
        </header>
        <DiffFiles files={files()} split={split()} onOpenLine={openAt} />
        <Show when={gitStore.diff?.truncated}>
          <p class="panel-note">Some files were over budget and are not listed.</p>
        </Show>
      </Show>
    </div>
  );
}
