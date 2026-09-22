import { For, Show, createMemo } from "solid-js";
import { openDiff, revealInTree } from "../../navigation/viewsStore";
import { beginSessionChanges, splitEntry } from "./sessionChangesStore";
import { Icon, LangIcon } from "../../theme/icons/index";
import { EmptyState, IconButton, Skeleton, Tooltip } from "../../ui/index";
import { baseNote, summaryTotals } from "../../contracts/workbench";
import { loadSessionChanges } from "./commands";
import { statusLetter, statusWord } from "./gitView";

// `sessionChangesStore` schedules quiet-period refreshes to avoid polling git.
export function SessionChangesPanel(props: { session: string }) {
  const entry = () => splitEntry(props.session);
  const summary = () => entry().changes?.summary ?? null;
  const totals = createMemo(() => summaryTotals(summary()));

  function refresh(): void {
    beginSessionChanges(props.session);
    void loadSessionChanges(props.session).catch(() => undefined);
  }

  function open(path: string): void {
    openDiff();
    revealInTree(path);
  }

  const note = () => {
    const changes = entry().changes;
    return changes ? baseNote(changes.origin, changes.summary.base) : null;
  };

  return (
    <div class="session-changes">
      <header class="session-changes-head">
        <span class="session-changes-branch">{summary()?.branch ?? "detached"}</span>
        <Show when={summary()?.base}>
          {(base) => (
            <Tooltip label="Everything since this session started">
              <span class="session-changes-base">base {base()}</span>
            </Tooltip>
          )}
        </Show>
        <span class="history-spacer" />
        <span class="git-counts">
          <span class="added">+{totals().additions}</span>
          <span class="deleted">−{totals().deletions}</span>
        </span>
        <IconButton
          label="Re-read this session's changes"
          size="xs"
          onClick={refresh}
          disabled={entry().loading}
        >
          <Icon name="refresh" class="forge-icon-muted" size={13} />
        </IconButton>
      </header>

      <Show when={note()}>{(text) => <p class="panel-note">{text()}</p>}</Show>
      <Show when={entry().error}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      {/* The honest caveat behind a session-scoped diff: two agents writing one
          checkout cannot be told apart by looking at the result. */}
      <Show when={(entry().changes?.sharing_sessions ?? 0) > 0}>
        <p class="panel-note">
          {entry().changes?.sharing_sessions} other session
          {entry().changes?.sharing_sessions === 1 ? " is" : "s are"} writing this checkout, so some
          of this may not be theirs.
        </p>
      </Show>

      <Show
        when={summary()}
        fallback={
          <Show
            when={!entry().loading}
            fallback={<Skeleton label="Reading this session's changes" rows={4} />}
          >
            <EmptyState message="No changes read yet." />
          </Show>
        }
      >
        {(current) => (
          <>
            <Show when={current().commit_count > 0}>
              <section class="session-changes-commits">
                <h4 class="panel-subhead">
                  {current().commit_count} commit{current().commit_count === 1 ? "" : "s"}
                </h4>
                <For each={current().commits}>
                  {(commit) => (
                    <div class="session-commit">
                      <span class="session-commit-id">{commit.short_id}</span>
                      <span class="session-commit-subject">{commit.subject}</span>
                    </div>
                  )}
                </For>
                <Show when={current().commits.length < current().commit_count}>
                  <p class="panel-note">
                    …and {current().commit_count - current().commits.length} more.
                  </p>
                </Show>
              </section>
            </Show>

            <Show
              when={current().files.length > 0}
              fallback={<EmptyState message="Nothing changed since this session started." />}
            >
              <section class="session-changes-files">
                <For each={current().files}>
                  {(file) => (
                    <button
                      type="button"
                      class="forge-row session-change-row"
                      onClick={() => open(file.path)}
                    >
                      <LangIcon path={file.path} size={13} />
                      <span
                        class="git-status"
                        data-status={file.status}
                        title={statusWord(file.status)}
                      >
                        {statusLetter(file.status)}
                      </span>
                      <span class="tree-label">{file.path}</span>
                      <span class="git-counts">
                        <Show
                          when={file.binary}
                          fallback={<span class="added">+{file.additions}</span>}
                        >
                          <span class="panel-note">bin</span>
                        </Show>
                        <Show when={!file.binary}>
                          <span class="deleted">−{file.deletions}</span>
                        </Show>
                      </span>
                    </button>
                  )}
                </For>
              </section>
            </Show>

            <Show when={current().truncated}>
              <p class="panel-note">Some entries were over budget and are not listed.</p>
            </Show>
          </>
        )}
      </Show>

      <footer class="session-changes-foot">
        <Show when={entry().readAt} fallback={<span class="panel-note">not read yet</span>}>
          {(at) => <span class="panel-note">as of {new Date(at()).toLocaleTimeString()}</span>}
        </Show>
      </footer>
    </div>
  );
}
