import { For, Show, createEffect, createMemo, createSignal, on } from "solid-js";
import { openEditorAt } from "../editor/open";
import { DIFF_SPLIT_KEY, readFlag, writeFlag } from "../../state/preferences";
import { LangIcon } from "../../theme/icons/index";
import { Button, EmptyState, Select, Skeleton } from "../../ui/index";
import { DiffFiles } from "./diff/DiffFiles";
import { diffTotals } from "../../contracts/workbench";
import { openFile } from "../files/commands";
import { listBranches, loadBranchCompare } from "./commands";
import { baseRefOptions, visibleFiles } from "./compareBase";
import { statusLetter, statusWord } from "./gitView";
import { gitStore } from "./state";
import { forgeStore } from "../../state/forgeStore";
import { loading } from "../../state/loading";

/** A pull request before it exists: the branch against its base, committed work only. */
export function CompareView(props: { workspace: string }) {
  const [split, setSplit] = createSignal(readFlag(DIFF_SPLIT_KEY, false));
  const [selected, setSelected] = createSignal<string | null>(null);

  const checkout = createMemo(() =>
    forgeStore.workspaces.find((item) => item.id === props.workspace),
  );
  const compare = () =>
    gitStore.compare?.workspace_id === props.workspace ? gitStore.compare : null;
  const files = createMemo(() => compare()?.diff.files ?? []);
  const shown = createMemo(() => visibleFiles(files(), selected()));
  const bases = createMemo(() =>
    baseRefOptions(gitStore.branches, checkout()?.branch ?? null, compare()?.base_ref ?? null),
  );

  createEffect(
    on(
      () => checkout()?.project_id,
      (project) => {
        if (project) void listBranches(project).catch(() => undefined);
      },
    ),
  );

  function reload(base: string | null = compare()?.base_ref ?? null): void {
    void loadBranchCompare(props.workspace, base).catch(() => undefined);
  }

  function toggleSplit(): void {
    const next = !split();
    setSplit(next);
    writeFlag(DIFF_SPLIT_KEY, next);
  }

  function openAt(path: string, line: number): void {
    openEditorAt(path, line);
    void openFile(props.workspace, path).catch(() => undefined);
  }

  return (
    <div class="compare-view">
      <header class="diff-header compare-header">
        <span class="compare-branch">{checkout()?.branch ?? "detached"}</span>
        <span aria-hidden="true">→</span>
        <Select
          class="compare-base"
          aria-label="Base branch"
          placeholder="base branch"
          value={compare()?.base_ref ?? null}
          options={bases().map((ref) => ({ value: ref, label: ref }))}
          onChange={(ref) => reload(ref)}
        />
        <Show when={compare()}>
          {(current) => (
            <>
              <span class="panel-note">
                {current().commit_count} commit{current().commit_count === 1 ? "" : "s"} ·{" "}
                {current().diff.files.length} file{current().diff.files.length === 1 ? "" : "s"}
              </span>
              <span class="git-counts">
                <span class="added">+{diffTotals(current().diff).additions}</span>
                <span class="deleted">−{diffTotals(current().diff).deletions}</span>
              </span>
            </>
          )}
        </Show>
        <span class="history-spacer" />
        <Button variant="secondary" size="xs" selected={split()} onClick={toggleSplit}>
          {split() ? "Split" : "Unified"}
        </Button>
        <Button variant="secondary" size="xs" onClick={() => reload()}>
          Reload
        </Button>
      </header>

      <Show when={gitStore.compareError}>{(error) => <p class="panel-error">{error()}</p>}</Show>

      <Show
        when={compare()}
        fallback={
          <Show when={!loading.compare} fallback={<Skeleton label="Comparing branches" rows={6} />}>
            <EmptyState message="Nothing compared yet." />
          </Show>
        }
      >
        {(current) => (
          <Show
            when={current().base_ref}
            fallback={
              <EmptyState message="No base branch resolves here: no origin default, main or master. Pick one above." />
            }
          >
            <Show
              when={files().length > 0}
              fallback={<EmptyState message="No committed changes against this base." />}
            >
              <div class="compare-body">
                <nav class="compare-rail" aria-label="Changed files">
                  <button
                    type="button"
                    class="forge-row compare-file"
                    classList={{ active: selected() === null }}
                    onClick={() => setSelected(null)}
                  >
                    <span class="tree-label">All files</span>
                  </button>
                  <For each={files()}>
                    {(file) => (
                      <button
                        type="button"
                        class="forge-row compare-file"
                        classList={{ active: selected() === file.path }}
                        aria-current={selected() === file.path ? "true" : undefined}
                        title={file.path}
                        onClick={() => setSelected(file.path)}
                      >
                        <LangIcon path={file.path} size={13} />
                        <span
                          class="git-status"
                          data-status={file.status}
                          title={statusWord(file.status)}
                        >
                          {statusLetter(file.status)}
                        </span>
                        <span class="tree-label">
                          {file.path.slice(file.path.lastIndexOf("/") + 1)}
                        </span>
                        <span class="git-counts">
                          <span class="added">+{file.additions}</span>
                          <span class="deleted">−{file.deletions}</span>
                        </span>
                      </button>
                    )}
                  </For>
                </nav>
                <div class="diff-view compare-diff">
                  <Show when={current().commits.length > 0}>
                    <details class="session-changes-commits">
                      <summary class="panel-subhead">
                        Commits since {current().merge_base ?? "the merge base"}
                      </summary>
                      <For each={current().commits}>
                        {(commit) => (
                          <div class="session-commit">
                            <span class="session-commit-id">{commit.short_id}</span>
                            <span class="session-commit-subject">{commit.subject}</span>
                          </div>
                        )}
                      </For>
                    </details>
                  </Show>
                  <DiffFiles files={shown()} split={split()} onOpenLine={openAt} />
                  <Show when={current().diff.truncated}>
                    <p class="panel-note">Some files were over budget and are not listed.</p>
                  </Show>
                </div>
              </div>
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );
}
