import { For, Show, createMemo, createSignal } from "solid-js";
import { openDiff } from "../../navigation/viewsStore";
import { loading, setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";
import { diffTotals } from "../../contracts/workbench";
import { Icon } from "../../theme/icons/index";
import { Button, IconButton } from "../../ui/index";
import { openComposeForWorkspace } from "../pull-requests/PrComposeView";
import { applyJuvaDraft, draftWithJuva, loadDiff } from "./commands";
import { commitLabel, commitMessage, firstHunk, statusLetter, statusWord } from "./gitView";
import { refreshGit } from "./rebaseActions";
import { gitStore } from "./state";

/**
 * The checkout's uncommitted work: the files, the first hunk of the one picked,
 * and a commit box.
 *
 * `CreateCommit` stages the whole tree, so the box commits every file listed
 * here and never pushes.
 */
export function WorkingTreeRail(props: { committable: boolean }) {
  const [picked, setPicked] = createSignal<string | null>(null);
  const [message, setMessage] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const files = () => gitStore.diff?.files ?? [];
  const totals = createMemo(() => diffTotals(gitStore.diff));
  const selected = createMemo(
    () => files().find((file) => file.path === picked()) ?? files()[0] ?? null,
  );
  const hunk = createMemo(() => {
    const file = selected();
    return file && !file.binary && !file.truncated ? firstHunk(file.patch) : null;
  });

  const canCommit = () =>
    props.committable && files().length > 0 && message().trim().length > 0 && !busy();

  function commit(): void {
    const workspace = activeWorkspace();
    if (!workspace || !canCommit()) return;
    const { title, body } = commitMessage(message());
    setBusy(true);
    setError(null);
    void applyJuvaDraft(workspace, "CommitMessage", title, body)
      .then(() => {
        setMessage("");
        setLoading("diff", true);
        return loadDiff(workspace);
      })
      .catch(() => setError("The daemon refused the commit."))
      .finally(() => setBusy(false));
  }

  function draft(): void {
    const workspace = activeWorkspace();
    if (workspace) void draftWithJuva(workspace, "CommitMessage").catch(() => undefined);
  }

  return (
    <section class="git-tree" aria-labelledby="git-tree-title">
      <header class="git-section-head">
        <h3 id="git-tree-title" class="forge-section-label">
          Working tree
        </h3>
        <span class="history-spacer" />
        <Show when={files().length > 0}>
          <span class="git-counts">
            <span class="added">+{totals().additions}</span>
            <span class="deleted">−{totals().deletions}</span>
          </span>
        </Show>
        <IconButton label="Re-read the working tree" size="xs" variant="ghost" onClick={refreshGit}>
          <Icon name="refresh" size={13} />
        </IconButton>
      </header>
      <Show when={gitStore.diffError}>{(message) => <p class="panel-error">{message()}</p>}</Show>

      <ul class="git-files">
        <For
          each={files()}
          fallback={
            <li class="empty-copy">
              {loading.diff ? "Reading the working tree…" : "Nothing uncommitted."}
            </li>
          }
        >
          {(file) => (
            <li>
              <button
                type="button"
                class="git-file"
                classList={{ "forge-selected": selected()?.path === file.path }}
                aria-pressed={selected()?.path === file.path}
                aria-label={`${file.path}, ${statusWord(file.status)}, ${file.additions} added, ${file.deletions} removed`}
                onClick={() => setPicked(file.path)}
              >
                <span class="git-status" data-status={file.status} aria-hidden="true">
                  {statusLetter(file.status)}
                </span>
                <span class="git-file-path">{file.path}</span>
                <span class="git-counts" aria-hidden="true">
                  <Show when={file.additions > 0}>
                    <span class="added">+{file.additions}</span>
                  </Show>
                  <Show when={file.deletions > 0}>
                    <span class="deleted">−{file.deletions}</span>
                  </Show>
                </span>
              </button>
            </li>
          )}
        </For>
      </ul>
      <Show when={gitStore.diff?.truncated}>
        <p class="panel-note">Some files were over budget and are not listed.</p>
      </Show>

      <Show when={selected()}>
        {(file) => (
          <Show
            when={hunk()}
            fallback={
              <p class="panel-note">
                {file().binary
                  ? "Binary file: no lines to preview."
                  : file().truncated
                    ? "This patch was over budget and is not shown."
                    : "No lines changed."}
              </p>
            }
          >
            {(preview) => (
              <figure class="git-hunk" aria-label={`First change in ${file().path}`}>
                <figcaption class="git-hunk-head">
                  <span class="git-hunk-range">{preview().header}</span>
                </figcaption>
                <div class="git-hunk-body">
                  <For each={preview().rows}>
                    {(row) => (
                      <div class="git-hunk-row" data-kind={row.kind}>
                        <span class="git-hunk-marker" aria-hidden="true">
                          {row.kind === "added" ? "+" : row.kind === "removed" ? "−" : " "}
                        </span>
                        <span class="git-hunk-text">{row.text}</span>
                      </div>
                    )}
                  </For>
                </div>
                <Show when={preview().hiddenRows > 0 || preview().more > 0}>
                  <p class="git-hunk-foot">
                    {[
                      preview().hiddenRows > 0 ? `${preview().hiddenRows} more lines` : null,
                      preview().more > 0
                        ? `${preview().more} more hunk${preview().more === 1 ? "" : "s"}`
                        : null,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </p>
                </Show>
              </figure>
            )}
          </Show>
        )}
      </Show>

      <div class="git-commit">
        <div class="git-commit-field">
          <label for="git-commit-message" class="forge-section-label">
            Message
          </label>
          <textarea
            id="git-commit-message"
            class="git-commit-input"
            rows={2}
            placeholder={props.committable ? "What this change does" : "Finish the replay first"}
            disabled={!props.committable}
            value={message()}
            onInput={(event) => setMessage(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (!(event.metaKey || event.ctrlKey) || event.key !== "Enter") return;
              event.preventDefault();
              commit();
            }}
          />
        </div>
        <div class="git-commit-actions">
          <Button
            variant="primary"
            class="git-commit-button"
            disabled={!canCommit()}
            loading={busy()}
            onClick={commit}
          >
            {commitLabel(files().length)}
          </Button>
          <IconButton
            label="Draft the message with Juva"
            variant="secondary"
            disabled={!props.committable || files().length === 0}
            onClick={draft}
          >
            <Icon name="edit" size={14} />
          </IconButton>
        </div>
        <p class="git-commit-note">
          {props.committable
            ? "Stages every change above, then commits. Nothing is pushed."
            : "Nothing can be committed while the replay is stopped."}
        </p>
        <Show when={error()}>{(message) => <p class="panel-error">{message()}</p>}</Show>
      </div>

      <div class="git-actions">
        <Button variant="secondary" size="sm" disabled={files().length === 0} onClick={openDiff}>
          Open patch
        </Button>
        <Show when={files().length > 0}>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              const workspace = activeWorkspace();
              if (workspace) openComposeForWorkspace(workspace);
            }}
          >
            Open PR with agent…
          </Button>
        </Show>
      </div>
    </section>
  );
}
