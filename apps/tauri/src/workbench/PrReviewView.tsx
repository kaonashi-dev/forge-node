import { Show, createEffect, createMemo, createSignal, onMount } from "solid-js";
import { newAgent, openUrl, selectSession, setAppState } from "../runtime/api";
import { sessionStateLabel, sessionTitle, type Session } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { setRuntimeStore } from "../store/runtimeStore";
import { workbenchStore } from "../store/workbenchStore";
import { adoptReviewSession, beginReview, clearReview, reviewRun } from "../store/prReviewStore";
import { close, focus, showTerminal } from "../store/viewsStore";
import { findPullRequest } from "./PrDetailView";
import {
  CUSTOM_RECIPE,
  REVIEW_CUSTOM_KEY,
  REVIEW_PROVIDER_KEY,
  REVIEW_RECIPE_KEY,
  adoptLaunched,
  builtinRecipes,
  composeReviewPrompt,
  launchHint,
  preferredAgent,
  readyToLaunch,
  recipeOrDefault,
  reviewAgents,
  reviewWorkspace,
  type ReviewRecipe,
} from "./prReview";
import { TERMINAL_VIEW } from "./views";
import { Badge, Button, EmptyState, RadioGroup, Select, TextArea, Tooltip } from "../ui";

/**
 * The first pass over someone else's pull request, run by an agent (§16.9).
 *
 * A tab and not a dialog: the agent takes minutes, and the point of the feature
 * is to start it and go on reading. Closing and re-opening it from the same
 * pull request comes back to the same run, because the tab is keyed on the
 * pull request rather than on the launch.
 *
 * The review is a *read*. It runs in a checkout that already exists, reaches
 * the pull request through `gh`, and is launched in the provider's own
 * read-only mode — so the branch under the user's working tree never moves and
 * nothing reaches GitHub that the user did not send themselves.
 */
export function PrReviewView(props: { prKey: string }) {
  const [recipeId, setRecipeId] = createSignal<string | null>(null);
  const [agentId, setAgentId] = createSignal<string | null>(null);
  const [custom, setCustom] = createSignal("");
  const [promptOpen, setPromptOpen] = createSignal(false);

  // The run lives in the store, not here: this component is unmounted the
  // moment the reader looks at another tab, and a running review that
  // disappeared with it would come back offering to start a second one.
  const run = () => reviewRun(props.prKey);

  const pr = createMemo(() => findPullRequest(props.prKey));
  const agents = createMemo(() => reviewAgents(forgeStore.providers));
  const agent = createMemo(() => preferredAgent(agents(), agentId()));
  const recipe = createMemo((): ReviewRecipe => recipeOrDefault(recipeId()));

  const workspace = createMemo(() =>
    reviewWorkspace(forgeStore.workspaces, pr()?.project_id ?? null, workbenchStore.workspace),
  );

  const prompt = createMemo(() => {
    const item = pr();
    if (!item) return "";
    return composeReviewPrompt(recipe(), item, recipe().id === CUSTOM_RECIPE ? custom() : "");
  });

  // The last choices, read once. Not an effect on `app_state`: a preference
  // that re-applied itself while the user was picking would fight the picker.
  onMount(() => {
    const state = forgeStore.app_state;
    setRecipeId(state[REVIEW_RECIPE_KEY] ?? null);
    setAgentId(state[REVIEW_PROVIDER_KEY] ?? null);
    setCustom(state[REVIEW_CUSTOM_KEY] ?? "");
  });

  /*
   * Start on open.
   *
   * Pressing Review *is* the request; making the reader press a second button
   * is the tab asking for permission it already has. Two guards keep it to
   * once: the store, so a tab brought back on screen does not spawn an agent
   * per glance, and `autoStarted`, so a launch the daemon refuses — which
   * clears the store entry — is not retried in a loop.
   *
   * An effect rather than `onMount` because the snapshot may still be arriving:
   * with no providers yet there is no agent to pick, and the tab would sit
   * there having quietly decided not to.
   */
  let autoStarted = false;
  createEffect(() => {
    if (autoStarted || started()) return;
    const picked = agent();
    const target = workspace();
    // Nothing to decide yet: with no providers in the snapshot there is no
    // agent to pick. Once there is one, the decision is made exactly once —
    // a remembered Custom recipe with nothing written in it stays waiting for
    // the button rather than firing on the first character typed.
    if (!picked || !target) return;
    autoStarted = true;
    if (readyToLaunch(picked, recipe(), custom(), false)) launch();
  });

  /*
   * Adopt the session the launch created.
   *
   * The runtime command channel is one-way — it also carries keystrokes, so it
   * cannot block on an answer — and the id therefore does not come back. The
   * recorded `startedAt` is what keeps this from adopting a session that was
   * already running in the same checkout.
   */
  createEffect(() => {
    const current = run();
    if (!current || current.session) return;
    const found = adoptLaunched(forgeStore.sessions, current.workspace, current.startedAt);
    if (found) adoptReviewSession(props.prKey, found.id);
  });

  const running = createMemo((): Session | null => {
    const id = run()?.session ?? null;
    return id ? (forgeStore.sessions.find((item) => item.id === id) ?? null) : null;
  });

  const awaiting = () => run() !== null && run()?.session == null;
  const started = () => run() !== null;

  function remember(key: string, value: string): void {
    void setAppState(key, value).catch(() => undefined);
  }

  function launch(): void {
    const item = pr();
    const target = workspace();
    const picked = agent();
    if (!item || !target || !picked) return;
    remember(REVIEW_RECIPE_KEY, recipe().id);
    remember(REVIEW_PROVIDER_KEY, picked.id);
    if (recipe().id === CUSTOM_RECIPE) remember(REVIEW_CUSTOM_KEY, custom());
    // Recorded before the command goes out, so a session that starts fast is
    // still newer than the moment we asked.
    beginReview(props.prKey, target.id);
    void newAgent(picked.id, null, target.id, null, prompt(), null, true).catch(() => {
      clearReview(props.prKey);
      setRuntimeStore("notice", "The daemon refused to launch the review.");
    });
  }

  /**
   * Start a second pass over the same pull request.
   *
   * The previous run is let go of first — the tab follows one review at a
   * time, and adopting the older session again would leave the new one running
   * with nothing pointing at it.
   */
  function relaunch(): void {
    clearReview(props.prKey);
    launch();
  }

  function showSession(): void {
    const id = run()?.session ?? null;
    if (id) void selectSession(id).catch(() => undefined);
    showTerminal();
    focus(TERMINAL_VIEW);
  }

  return (
    <Show when={pr()} fallback={<EmptyState message="Pull request not found in the last read." />}>
      {(item) => (
        <div class="pr-review">
          <header class="pr-review-head">
            <h2>
              <span class="pr-number">#{item().number}</span> {item().title}
            </h2>
            <div class="pr-detail-actions">
              <Show when={item().is_draft}>
                <Badge>draft</Badge>
              </Show>
              <Button
                variant="secondary"
                onClick={() => void openUrl(item().url).catch(() => undefined)}
              >
                Open in browser
              </Button>
              <Button
                variant="secondary"
                onClick={() => close({ kind: "pr_review", key: props.prKey })}
              >
                Close
              </Button>
            </div>
          </header>
          <p class="panel-note">
            {item().repository} · {item().head_ref} → {item().base_ref} · {item().changed_files}{" "}
            file
            {item().changed_files === 1 ? "" : "s"} · +{item().additions} −{item().deletions}
          </p>

          {/* The one case a review cannot start: `gh` needs a checkout whose
              remote is this repository to be authenticated against it. */}
          <Show when={item().project_id === null}>
            <p class="panel-error">
              This pull request is from a repository Forge does not have. Add the project to review
              it here.
            </p>
          </Show>

          <div class="pr-review-body">
            <section class="pr-compose-tasks">
              <RadioGroup
                label="What to ask for"
                itemClass="pr-task-row"
                value={recipe().id}
                onChange={setRecipeId}
                options={builtinRecipes().map((option) => ({
                  value: option.id,
                  label: option.name,
                  render: () => (
                    <>
                      <span class="pr-task-name">{option.name}</span>
                      <span class="pr-task-detail">{option.detail}</span>
                    </>
                  ),
                }))}
              />
            </section>

            <Show when={recipe().id === CUSTOM_RECIPE}>
              <TextArea
                class="settings-hint"
                label="Ask for"
                rows={4}
                value={custom()}
                onChange={setCustom}
                placeholder="What should the agent look for?"
              />
            </Show>

            <section class="pr-review-agent">
              <Select
                label="Agent"
                aria-label="Agent to review with"
                value={agent()?.id ?? null}
                placeholder="No agent can review"
                options={agents().map((option) => ({
                  value: option.id,
                  label: `${option.name} · ${option.mode}`,
                }))}
                onChange={setAgentId}
              />
              <Tooltip label="Show prompt" contents>
                <Button
                  variant="secondary"
                  class={promptOpen() ? "active" : ""}
                  aria-expanded={promptOpen()}
                  aria-label="Show prompt"
                  onClick={() => setPromptOpen((open) => !open)}
                >
                  ⚙
                </Button>
              </Tooltip>
            </section>

            <Show when={promptOpen()}>
              <section class="pr-compose-prompt">
                <p class="settings-hint">This is what the agent will be sent.</p>
                <pre class="pr-prompt-preview">{prompt()}</pre>
              </section>
            </Show>

            <section class="pr-compose-launch">
              <Button
                variant="primary"
                disabled={
                  !readyToLaunch(agent(), recipe(), custom(), awaiting() || running() !== null) ||
                  workspace() === null
                }
                onClick={launch}
              >
                {awaiting() ? "Starting…" : "Start review"}
              </Button>
              <Show when={started()}>
                <Button variant="secondary" onClick={relaunch}>
                  Review again
                </Button>
              </Show>
              <p class="settings-hint pr-compose-hint">
                {launchHint(agent(), recipe(), custom(), awaiting() || running() !== null)}
              </p>
            </section>

            <Show when={running() !== null || awaiting()}>
              <button type="button" class="forge-row pr-compose-progress" onClick={showSession}>
                <span>
                  <Show when={running()} fallback="starting the agent…">
                    {(item) => `${sessionTitle(item())} · ${sessionStateLabel(item().state)}`}
                  </Show>
                </span>
                <span class="settings-row-note">open the session</span>
              </button>
            </Show>
          </div>
        </div>
      )}
    </Show>
  );
}
