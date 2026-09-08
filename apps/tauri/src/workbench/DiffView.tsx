import { Show, createMemo, createSignal } from "solid-js";
import { workbenchStore } from "../store/workbenchStore";
import { openEditorAt } from "../store/viewsStore";
import { openFile } from "./api";
import { DIFF_SPLIT_KEY, readFlag, writeFlag } from "../shell/layout";
import { Button, EmptyState, Skeleton } from "../ui";
import { DiffFiles } from "./diff/DiffFiles";
import { diffTotals } from "./types";

/**
 * The uncommitted changes of a checkout, one patch per file (§16.7).
 *
 * A read, not a subscription: what is on screen is the answer to a `LoadDiff`
 * the Git panel asked for, and it goes stale the moment anything writes a file.
 * The refresh is deliberate rather than automatic for exactly that reason — a
 * diff that redrew itself under an agent's edits would be unreadable.
 */
export function DiffView() {
  const [split, setSplit] = createSignal(readFlag(DIFF_SPLIT_KEY, false));

  const files = createMemo(() => workbenchStore.diff?.files ?? []);

  function toggleSplit(): void {
    const next = !split();
    setSplit(next);
    writeFlag(DIFF_SPLIT_KEY, next);
  }

  /** D4: open the file at the line the patch was read at. */
  function openAt(path: string, line: number): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    // The read is started here as well as by the editor's own effect: the tab
    // is about to mount, and one round trip earlier is the difference between
    // landing on the line and landing on an empty buffer that then jumps.
    openEditorAt(path, line);
    void openFile(workspace, path).catch(() => undefined);
  }

  return (
    <div class="diff-view">
      <Show when={workbenchStore.diffError}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      <Show
        when={workbenchStore.diff && workbenchStore.diff.files.length > 0}
        fallback={
          <Show
            when={!workbenchStore.loading.diff}
            fallback={<Skeleton label="Reading the diff" rows={6} />}
          >
            <EmptyState
              message="Nothing uncommitted."
              actions={[{ action: "toggle_git", label: "Open the Git panel", icon: "git-branch" }]}
            />
          </Show>
        }
      >
        <header class="diff-header">
          <span>{workbenchStore.diff?.branch ?? "detached"}</span>
          <span class="git-counts">
            <span class="added">+{diffTotals(workbenchStore.diff).additions}</span>
            <span class="deleted">−{diffTotals(workbenchStore.diff).deletions}</span>
          </span>
          <span class="history-spacer" />
          <Button variant="secondary" size="xs" selected={split()} onClick={toggleSplit}>
            {split() ? "Split" : "Unified"}
          </Button>
        </header>
        <DiffFiles files={files()} split={split()} onOpenLine={openAt} />
        <Show when={workbenchStore.diff?.truncated}>
          <p class="panel-note">Some files were over budget and are not listed.</p>
        </Show>
      </Show>
    </div>
  );
}
