import { For, Show, createMemo, onMount } from "solid-js";
import { openDiff } from "../store/viewsStore";
import { openComposeForWorkspace } from "../workbench/PrComposeView";
import { setLoading, workbenchStore } from "../store/workbenchStore";
import { forgeStore } from "../store/forgeStore";
import { requestConfirm, setRuntimeStore } from "../store/runtimeStore";
import { newAgent } from "../runtime/api";
import {
  abortRebase,
  continueRebase,
  loadDiff,
  loadRebaseState,
  markConflictResolved,
} from "../workbench/api";
import { conflictLabel, conflictPrompt, resolverFor } from "../workbench/conflictPrompt";
import { diffTotals, rebaseInProgress, readyToContinue } from "../workbench/types";
import { Button, Tooltip } from "../ui";

/**
 * The Git tab: uncommitted changes, and a stopped rebase when there is one.
 *
 * A stopped rebase is **read, never remembered**: every action here re-reads
 * the state from the index and git's own files, which is why nothing caches a
 * conflict count. Staging a path is what takes it off the list.
 */
export function GitPanel() {
  function refresh(): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    setLoading("diff", true);
    setLoading("rebase", true);
    void loadDiff(workspace).catch(() => undefined);
    void loadRebaseState(workspace).catch(() => undefined);
  }

  function act(run: (workspace: string) => Promise<void>): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    setLoading("rebase", true);
    void run(workspace)
      .then(() => {
        // The sequencer moved, so the working tree did too.
        setLoading("diff", true);
        return loadDiff(workspace);
      })
      .catch(() => undefined);
  }

  const rebase = () => workbenchStore.rebase;
  const workspace = createMemo(() =>
    forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace),
  );
  const unresolved = () => rebase()?.conflicts.length ?? 0;

  /**
   * "Resolve with AI" is the ordinary launch.
   *
   * The same `CreateAgentSession` the rail's `+` sends, in the same workspace,
   * with the message already typed. An agent that cannot be handed a prompt is
   * refused here — with a sentence naming what to change — rather than at the
   * daemon, where it would arrive as a launch that did nothing.
   */
  function resolveWithAgent(): void {
    const target = workbenchStore.workspace;
    const state = rebase();
    if (!target || !state) return;
    const resolver = resolverFor(forgeStore);
    if (resolver.kind === "unavailable") {
      setRuntimeStore("notice", resolver.reason);
      return;
    }
    void newAgent(resolver.provider, resolver.profile, target, null, conflictPrompt(state)).catch(
      () => undefined,
    );
  }

  /* Aborting throws every resolution away, so it asks first. Continuing does
     not: it only moves a replay whose conflicts are already staged. */
  function abort(): void {
    const count = unresolved();
    requestConfirm({
      title: `Abort the ${rebase()?.operation ?? "rebase"}?`,
      description:
        count > 0
          ? `${count} unresolved path(s) will be lost, along with every resolution made so far.`
          : "Every resolution made so far is thrown away.",
      confirmLabel: "Abort",
      destructive: true,
      onConfirm: () => act(abortRebase),
    });
  }

  onMount(refresh);

  return (
    <div class="panel-body">
      {/* The three facts a git panel opens with: the branch, that HEAD is
          detached while a replay is in flight — which is *why* nothing can be
          committed — and how far the checkout is from its upstream. */}
      <Show when={workspace()}>
        {(checkout) => (
          <div class="git-head">
            <Show
              when={rebaseInProgress(rebase()) && rebase()?.head}
              fallback={<span class="git-branch">{checkout().branch ?? checkout().path}</span>}
            >
              {(head) => (
                <Tooltip label="HEAD is detached while the replay is in flight" contents>
                  <span class="git-detached">Detached HEAD · {head().slice(0, 7)}</span>
                </Tooltip>
              )}
            </Show>
            <Show when={checkout().status.ahead !== null || checkout().status.behind !== null}>
              <span class="panel-note-inline">
                ↑{checkout().status.ahead ?? 0} ↓{checkout().status.behind ?? 0}
              </span>
            </Show>
          </div>
        )}
      </Show>
      <Show when={workbenchStore.rebaseError}>
        {(error) => <p class="panel-error">{error()}</p>}
      </Show>

      <Show when={rebaseInProgress(rebase())}>
        <section class="git-sequencer">
          <div class="git-sequencer-head">
            <span class="git-sequencer-title">
              {rebase()?.operation} conflicts: {unresolved()} unresolved
            </span>
            <Show when={rebase()?.step && rebase()?.total}>
              <span class="panel-note-inline">
                {rebase()?.step}/{rebase()?.total}
              </span>
            </Show>
          </div>
          <p class="settings-hint">
            {rebase()?.branch} onto {rebase()?.onto} — a file goes back to being an ordinary change
            as soon as it is staged.
          </p>

          {/* The four ways out, in the order they are recommended. */}
          <div class="git-actions">
            <Button variant="primary" size="sm" onClick={resolveWithAgent}>
              Resolve with AI
            </Button>
            <Button variant="secondary" size="sm" onClick={openDiff}>
              Review conflicts
            </Button>
            {/* Only once every path is staged: the list is what the daemon
                just read, never a count kept between reads. */}
            <Show when={readyToContinue(rebase())}>
              <Button variant="secondary" size="sm" onClick={() => act(continueRebase)}>
                Continue {rebase()?.operation?.toLowerCase()}
              </Button>
            </Show>
            <Button variant="secondary" size="sm" onClick={abort}>
              Abort {rebase()?.operation?.toLowerCase()}
            </Button>
          </div>

          <For each={rebase()?.conflicts}>
            {(conflict) => (
              <div class="git-conflict">
                {/* Two lines, because one does not fit: a path long enough to
                    matter and "deleted by us, modified by them" side by side in
                    a 320px panel left neither of them readable. */}
                <Tooltip label={conflict.path} contents>
                  <span class="git-conflict-path">{conflict.path}</span>
                </Tooltip>
                <span class="git-conflict-foot">
                  <Tooltip label={conflict.code} contents>
                    <span class="git-conflict-code">{conflictLabel(conflict.code)}</span>
                  </Tooltip>
                  <Button
                    variant="secondary"
                    size="xs"
                    onClick={() => act((w) => markConflictResolved(w, [conflict.path]))}
                  >
                    Mark resolved
                  </Button>
                </span>
              </div>
            )}
          </For>
          <Show when={rebase()?.truncated}>
            <p class="panel-note">
              More conflicts than the read reports; `git status` has the rest.
            </p>
          </Show>
        </section>
      </Show>

      <div class="panel-heading">
        Changes
        <Show when={workbenchStore.diff}>
          <span class="panel-note-inline">
            +{diffTotals(workbenchStore.diff).additions} −
            {diffTotals(workbenchStore.diff).deletions}
          </span>
        </Show>
        <Show when={workbenchStore.workspace && (workbenchStore.diff?.files.length ?? 0) > 0}>
          <Button
            variant="secondary"
            size="xs"
            onClick={() => {
              const workspace = workbenchStore.workspace;
              if (workspace) openComposeForWorkspace(workspace);
            }}
          >
            Open PR with agent…
          </Button>
        </Show>
      </div>
      <Show when={workbenchStore.diffError}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      <For
        each={workbenchStore.diff?.files}
        fallback={
          <p class="empty-copy">
            {workbenchStore.loading.diff ? "Reading the diff…" : "Nothing uncommitted."}
          </p>
        }
      >
        {(file) => (
          <div class="git-file">
            <span class="git-status" data-status={file.status}>
              {file.status.slice(0, 1)}
            </span>
            <span class="tree-label">{file.path}</span>
            <span class="git-counts">
              <span class="added">+{file.additions}</span>
              <span class="deleted">−{file.deletions}</span>
            </span>
          </div>
        )}
      </For>
      {/* A patch over budget is dropped whole and flagged, never cut: half a
          patch is not a patch. */}
      <Show when={workbenchStore.diff?.truncated}>
        <p class="panel-note">Some patches were over budget and are not shown.</p>
      </Show>
      <div class="git-actions">
        <Button variant="secondary" onClick={refresh}>
          Refresh
        </Button>
        <Button
          variant="secondary"
          disabled={!workbenchStore.diff?.files.length}
          onClick={openDiff}
        >
          Open patch
        </Button>
      </div>
    </div>
  );
}
