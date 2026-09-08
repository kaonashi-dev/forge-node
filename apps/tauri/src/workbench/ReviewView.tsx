import { For, Show, createMemo, createSignal } from "solid-js";
import { workbenchStore } from "../store/workbenchStore";
import { openEditorAt } from "../store/viewsStore";
import { draftWithJuva, openFile } from "./api";
import { DIFF_SPLIT_KEY, readFlag, writeFlag } from "../shell/layout";
import { SessionGlyph } from "../theme/icons";
import { Badge, Button, EmptyState, Skeleton } from "../ui";
import { DiffFiles } from "./diff/DiffFiles";
import { baseNote, diffTotals } from "./types";

/**
 * Everything this checkout changed since its sessions began (§16.7).
 *
 * One diff, not one per session: two agents writing the same files cannot be
 * told apart by looking at the result, so the sessions are a header over a
 * single answer rather than sections each claiming a share of it.
 *
 * The Juva summary is deliberately *not* part of the same read. It opens a
 * socket, so it arrives on its own event — the diff is useful before the prose
 * lands and stays useful if the prose never does.
 */
export function ReviewView(props: { workspace: string }) {
  const [split, setSplit] = createSignal(readFlag(DIFF_SPLIT_KEY, false));

  const review = () =>
    workbenchStore.review?.workspace_id === props.workspace ? workbenchStore.review : null;
  const files = createMemo(() => review()?.diff.files ?? []);
  const draft = () =>
    workbenchStore.juvaDraft?.kind === "ChangeReview" ? workbenchStore.juvaDraft : null;

  function toggleSplit(): void {
    const next = !split();
    setSplit(next);
    writeFlag(DIFF_SPLIT_KEY, next);
  }

  /** D4: the line lands through the editor's own `revealLine` once it opens. */
  function openAt(path: string, line: number): void {
    openEditorAt(path, line);
    void openFile(props.workspace, path).catch(() => undefined);
  }

  function regenerate(): void {
    void draftWithJuva(props.workspace, "ChangeReview").catch(() => undefined);
  }

  const note = () => {
    const current = review();
    return current ? baseNote(current.origin, current.base) : null;
  };

  return (
    <div class="diff-view review-view">
      <Show when={workbenchStore.reviewError}>
        {(error) => <p class="panel-error">{error()}</p>}
      </Show>
      <Show
        when={review()}
        fallback={
          <Show
            when={!workbenchStore.loading.review}
            fallback={<Skeleton label="Reading the checkout" rows={6} />}
          >
            <EmptyState message="Nothing to review yet." />
          </Show>
        }
      >
        {(current) => (
          <>
            <header class="diff-header">
              <span>{current().diff.branch ?? "detached"}</span>
              <Show when={current().base}>
                {(base) => <span class="session-changes-base">base {base()}</span>}
              </Show>
              <span class="git-counts">
                <span class="added">+{diffTotals(current().diff).additions}</span>
                <span class="deleted">−{diffTotals(current().diff).deletions}</span>
              </span>
              <span class="history-spacer" />
              <Show when={workbenchStore.reviewAt}>
                {(at) => (
                  <span class="panel-note">generated {new Date(at()).toLocaleTimeString()}</span>
                )}
              </Show>
              <Button variant="secondary" size="xs" selected={split()} onClick={toggleSplit}>
                {split() ? "Split" : "Unified"}
              </Button>
            </header>

            <Show when={note()}>{(text) => <p class="panel-note">{text()}</p>}</Show>

            <section class="review-sessions">
              <h4 class="panel-subhead">
                {current().sessions.length} session
                {current().sessions.length === 1 ? "" : "s"} wrote this checkout
              </h4>
              <For each={current().sessions}>
                {(session) => (
                  <div class="forge-row review-session-row">
                    <SessionGlyph providerId={session.provider} size={14} />
                    <span class="tree-label">{session.title}</span>
                    <Show when={session.active}>
                      <Badge tone="good">running</Badge>
                    </Show>
                    <Show when={session.base}>
                      {(base) => <span class="session-changes-base">{base()}</span>}
                    </Show>
                    <span class="panel-note">
                      {session.commit_count} commit{session.commit_count === 1 ? "" : "s"}
                    </span>
                  </div>
                )}
              </For>
            </section>

            <section class="review-summary">
              <header class="forge-row">
                <h4 class="panel-subhead">Summary</h4>
                <span class="history-spacer" />
                <Button variant="secondary" size="xs" onClick={regenerate}>
                  Regenerate
                </Button>
              </header>
              <Show
                when={draft()}
                fallback={
                  <Show
                    when={!workbenchStore.loading.juva}
                    fallback={<Skeleton label="Writing the summary" rows={3} />}
                  >
                    <p class="panel-note">No summary yet.</p>
                  </Show>
                }
              >
                {(text) => (
                  <>
                    <p class="review-summary-title">{text().title}</p>
                    <Show when={text().body}>
                      <pre class="review-summary-body">{text().body}</pre>
                    </Show>
                  </>
                )}
              </Show>
              <Show when={workbenchStore.juvaError}>
                {(error) => <p class="panel-error">{error()}</p>}
              </Show>
            </section>

            <Show when={current().commits.length > 0}>
              <section class="session-changes-commits">
                <h4 class="panel-subhead">Commits</h4>
                <For each={current().commits}>
                  {(commit) => (
                    <div class="session-commit">
                      <span class="session-commit-id">{commit.short_id}</span>
                      <span class="session-commit-subject">{commit.subject}</span>
                    </div>
                  )}
                </For>
              </section>
            </Show>

            <Show
              when={files().length > 0}
              fallback={<EmptyState message="Nothing changed since these sessions began." />}
            >
              <DiffFiles files={files()} split={split()} onOpenLine={openAt} />
            </Show>
            <Show when={current().diff.truncated}>
              <p class="panel-note">Some files were over budget and are not listed.</p>
            </Show>
          </>
        )}
      </Show>
    </div>
  );
}
