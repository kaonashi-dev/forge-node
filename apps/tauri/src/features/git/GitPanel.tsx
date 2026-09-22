import { For, Index, Show, createMemo, onMount } from "solid-js";
import { forgeStore } from "../../state/forgeStore";
import { activeWorkspace } from "../../state/workspace";
import { rebaseInProgress, readyToContinue } from "../../contracts/workbench";
import { Button, Tooltip } from "../../ui/index";
import { conflictLabel } from "./conflictPrompt";
import { conflictBlocksOf } from "./conflictBlocks";
import { operationName, rebaseSteps } from "./gitView";
import { ConflictMark } from "./GitMarks";
import {
  abortReplay,
  continueReplay,
  refreshGit,
  resolveWithAgent,
  reviewConflict,
  stageConflict,
} from "./rebaseActions";
import { WorkingTreeRail } from "./WorkingTreeRail";
import { gitStore } from "./state";

/** The Git tab: where the checkout stands, a stopped replay, and the working tree. */
export function GitPanel() {
  const rebase = () => gitStore.rebase;
  const replaying = () => rebaseInProgress(rebase());
  const workspace = createMemo(() =>
    forgeStore.workspaces.find((item) => item.id === activeWorkspace()),
  );
  const unresolved = () => rebase()?.conflicts.length ?? 0;
  const steps = createMemo(() => rebaseSteps(rebase()));
  const operation = () => operationName(rebase()?.operation);

  /** Marked regions per conflicted path, read off the diff already in hand. */
  const markers = createMemo(() => {
    const counts = new Map<string, number>();
    for (const file of gitStore.diff?.files ?? []) {
      if (file.status !== "Conflicted" || file.binary || file.truncated) continue;
      counts.set(file.path, conflictBlocksOf(file.patch).length);
    }
    return counts;
  });

  const focused = () => gitStore.conflictFocus ?? rebase()?.conflicts[0]?.path ?? null;

  onMount(refreshGit);

  return (
    <div class="panel-body git-panel">
      <Show when={workspace()}>
        {(checkout) => (
          <header class="git-head">
            <div class="git-head-row">
              <Show
                when={replaying() && rebase()?.head}
                fallback={<span class="git-branch">{checkout().branch ?? checkout().path}</span>}
              >
                {(head) => (
                  <span class="git-detached">
                    <svg
                      width="12"
                      height="12"
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.5"
                      stroke-linecap="round"
                      aria-hidden="true"
                    >
                      <path d="M8 2.4v3.2M8 10.4v3.2M2.4 8h3.2M10.4 8h3.2" />
                      <circle cx="8" cy="8" r="2.1" />
                    </svg>
                    detached · {head().slice(0, 7)}
                  </span>
                )}
              </Show>
              <span class="history-spacer" />
              <Show when={checkout().status.ahead !== null || checkout().status.behind !== null}>
                <span
                  class="git-upstream"
                  aria-label={`${checkout().status.ahead ?? 0} ahead, ${checkout().status.behind ?? 0} behind the upstream`}
                >
                  ↑{checkout().status.ahead ?? 0} ↓{checkout().status.behind ?? 0}
                </span>
              </Show>
            </div>
            <Show when={replaying()}>
              <p class="git-head-note">
                HEAD is detached while the replay is in flight — that is why nothing can be
                committed yet.
              </p>
            </Show>
          </header>
        )}
      </Show>
      <Show when={gitStore.rebaseError}>{(error) => <p class="panel-error">{error()}</p>}</Show>

      <Show when={replaying()}>
        <section
          class="git-sequencer"
          data-state={unresolved() > 0 ? "conflicted" : "ready"}
          aria-labelledby="git-sequencer-title"
        >
          <div class="git-sequencer-head">
            <Show
              when={unresolved() > 0}
              fallback={
                <span class="git-sequencer-ready" aria-hidden="true">
                  ✓
                </span>
              }
            >
              <span class="forge-attention-dot" aria-hidden="true" />
            </Show>
            <span id="git-sequencer-title" class="git-sequencer-title">
              {operation()} ·{" "}
              {unresolved() > 0 ? `${unresolved()} unresolved` : "ready to continue"}
            </span>
            <span class="history-spacer" />
            <Show when={steps()}>
              {(current) => <span class="git-sequencer-step">{current().label}</span>}
            </Show>
          </div>

          <Show when={steps()}>
            {(current) => (
              <div
                class="git-meter"
                role="progressbar"
                aria-label={`${operation()} progress`}
                aria-valuemin={1}
                aria-valuemax={rebase()?.total ?? 1}
                aria-valuenow={rebase()?.step ?? 1}
                aria-valuetext={current().label}
              >
                <Index each={current().segments}>
                  {(segment) => <span class="git-meter-step" data-state={segment()} />}
                </Index>
              </div>
            )}
          </Show>

          <Show when={rebase()?.branch && rebase()?.onto}>
            <p class="git-sequencer-branches">
              {rebase()?.branch} onto {rebase()?.onto}
            </p>
          </Show>

          <div class="git-actions">
            <Show when={unresolved() > 0}>
              <Button variant="primary" size="sm" onClick={resolveWithAgent}>
                Resolve with an agent
              </Button>
              <Button variant="secondary" size="sm" onClick={() => reviewConflict(focused())}>
                Review conflicts
              </Button>
            </Show>
            {/* Disabled rather than hidden: the button is where a reader looks
                for the way out, and the tooltip says what unlocks it. */}
            <Tooltip
              label={
                readyToContinue(rebase())
                  ? `Continue the ${operation().toLowerCase()}`
                  : "Stage every conflicted path first"
              }
              contents
            >
              <Button
                variant={readyToContinue(rebase()) ? "primary" : "secondary"}
                size="sm"
                disabled={!readyToContinue(rebase())}
                onClick={continueReplay}
              >
                Continue {operation().toLowerCase()}
              </Button>
            </Tooltip>
            <Button variant="ghost" size="sm" onClick={abortReplay}>
              Abort
            </Button>
          </div>
        </section>

        <Show when={unresolved() > 0}>
          <div class="git-section-head">
            <h3 class="forge-section-label">Conflicts</h3>
            <span class="git-conflict-count" aria-label={`${unresolved()} unresolved`}>
              {unresolved()}
            </span>
          </div>
          <ul class="git-conflicts">
            <For each={rebase()?.conflicts}>
              {(conflict) => {
                const regions = () => markers().get(conflict.path);
                return (
                  <li
                    class="git-conflict"
                    classList={{ "forge-attention-row": focused() === conflict.path }}
                  >
                    <button
                      type="button"
                      class="git-conflict-open"
                      aria-current={focused() === conflict.path ? "true" : undefined}
                      onClick={() => reviewConflict(conflict.path)}
                    >
                      <ConflictMark />
                      <span class="git-conflict-text">
                        <Tooltip label={conflict.path} contents>
                          <span class="git-conflict-path">{conflict.path}</span>
                        </Tooltip>
                        <span class="git-conflict-code">
                          {conflictLabel(conflict.code)}
                          <Show when={regions() !== undefined}>
                            {" · "}
                            {regions() === 0
                              ? "no markers left"
                              : `${regions()} marked region${regions() === 1 ? "" : "s"}`}
                          </Show>
                        </span>
                      </span>
                    </button>
                    <Button
                      variant="secondary"
                      size="xs"
                      aria-label={`Stage ${conflict.path}`}
                      onClick={() => stageConflict(conflict.path, regions() ?? 0)}
                    >
                      Stage
                    </Button>
                  </li>
                );
              }}
            </For>
          </ul>
          <Show when={rebase()?.truncated}>
            <p class="panel-note">
              More conflicts than the read reports; `git status` has the rest.
            </p>
          </Show>
        </Show>
      </Show>

      <WorkingTreeRail committable={!replaying()} />

      <Show when={replaying()}>
        <p class="git-foot">a staged file is just a change again</p>
      </Show>
    </div>
  );
}
