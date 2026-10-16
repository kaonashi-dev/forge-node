import { For, Show, createSignal } from "solid-js";
import { centerMode, currentViews, openCompare, openDiff } from "../../navigation/viewsStore";
import { loading, setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";
import { Icon } from "../../theme/icons/index";
import { Button, IconButton, Kbd, TextArea } from "../../ui/index";
import { openComposeForWorkspace } from "../pull-requests/PrComposeView";
import { applyJuvaDraft, draftWithJuva, loadBranchCompare, loadDiff } from "./commands";
import { commitLabel, commitMessage, statusLetter, statusWord } from "./gitView";
import { refreshGit } from "./rebaseActions";
import { gitStore } from "./state";
import type { Chord } from "../../actions/keys";

/** Shown on the button only; the textarea's own key handler commits. */
const COMMIT_CHORD: Chord = {
  key: "enter",
  code: "Enter",
  ctrl: false,
  alt: false,
  shift: false,
  meta: true,
};

/**
 * The checkout's uncommitted work: the files, then a commit box. A file opens
 * the patch as a tab; the rail itself never draws a diff.
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
  const onPatch = () => centerMode() === "code" && currentViews().active.kind === "diff";

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
          Changes
        </h3>
        <span class="history-spacer" />
        <Show when={files().length > 0}>
          <span class="git-section-count">{files().length}</span>
        </Show>
        <IconButton label="Re-read the working tree" size="xs" onClick={refreshGit}>
          <Icon name="refresh" size={13} class="forge-icon-muted" />
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
          {(file) => {
            const slash = file.path.lastIndexOf("/");
            return (
              <li>
                <button
                  type="button"
                  class="forge-row git-file"
                  aria-current={onPatch() && picked() === file.path ? "true" : undefined}
                  aria-label={`${file.path}, ${statusWord(file.status)}, ${file.additions} added, ${file.deletions} removed`}
                  onClick={() => {
                    setPicked(file.path);
                    openDiff();
                  }}
                >
                  <span class="git-status" data-status={file.status} aria-hidden="true">
                    {statusLetter(file.status)}
                  </span>
                  <span class="git-file-path forge-truncate">
                    {file.path.slice(slash + 1)}
                    <Show when={slash > 0}>
                      <span class="git-file-dir"> {file.path.slice(0, slash)}</span>
                    </Show>
                  </span>
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
            );
          }}
        </For>
      </ul>
      <Show when={gitStore.diff?.truncated}>
        <p class="panel-note">Some files were over budget and are not listed.</p>
      </Show>

      <span class="git-tree-spacer" />

      <div class="git-commit">
        <TextArea
          label="Commit message"
          rows={3}
          placeholder={props.committable ? "What this change does" : "Finish the replay first"}
          disabled={!props.committable}
          value={message()}
          onChange={setMessage}
          onKeyDown={(event) => {
            if (!(event.metaKey || event.ctrlKey) || event.key !== "Enter") return;
            event.preventDefault();
            commit();
          }}
        />
        <div class="git-commit-actions">
          <Button
            variant="primary"
            size="lg"
            class="git-commit-button"
            disabled={!canCommit()}
            loading={busy()}
            onClick={commit}
          >
            {commitLabel(files().length)}
            <Show when={files().length > 0}>
              <Kbd chord={COMMIT_CHORD} bare class="git-commit-chord" />
            </Show>
          </Button>
          <IconButton
            label="Draft the message with Juva"
            variant="secondary"
            size="lg"
            disabled={!props.committable || files().length === 0}
            onClick={draft}
          >
            <Icon name="sparkles" size={15} />
          </IconButton>
        </div>
        <p class="forge-caption git-commit-note">
          {props.committable
            ? "Stages every change above, then commits. Nothing is pushed."
            : "Nothing can be committed while the replay is stopped."}
        </p>
        <Show when={error()}>{(message) => <p class="panel-error">{message()}</p>}</Show>
        <div class="git-actions">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              const workspace = activeWorkspace();
              if (workspace) {
                openCompare(workspace);
                void loadBranchCompare(workspace).catch(() => undefined);
              }
            }}
          >
            Preview PR
          </Button>
          <Show when={files().length > 0}>
            <Button
              variant="ghost"
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
      </div>
    </section>
  );
}
