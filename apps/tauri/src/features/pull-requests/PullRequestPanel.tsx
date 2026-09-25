import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import { setAppState } from "../settings/commands";
import { forgeStore } from "../../state/forgeStore";
import { currentViews, openPrDetail, openPrReview } from "../../navigation/viewsStore";
import { prKey } from "./PrDetailView";
import {
  PR_SCOPES,
  PR_SCOPE_KEY,
  filterPullRequests,
  parseScope,
  scopeCounts,
  failureCopy,
  scopeEmptyCopy,
  scopeLabel,
  type PrScope,
} from "./prFilters";
import { decisionChip } from "./prDetail";
import { Icon } from "../../theme/icons/index";
import { Badge, Button, IconButton, ListCard, RadioGroup, SearchField } from "../../ui/index";
import { refreshPullRequests } from "./commands";
import { loading, setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";
import type { PullRequest } from "../../contracts/runtime";

// Snapshot data is cached; remote refreshes complete through events.
export function PullRequestPanel() {
  const [scope, setScope] = createSignal<PrScope>("all");
  const [query, setQuery] = createSignal("");

  const state = () => forgeStore.pull_requests;

  onMount(() => setScope(parseScope(forgeStore.app_state[PR_SCOPE_KEY])));

  function pickScope(next: string): void {
    const parsed = parseScope(next);
    setScope(parsed);
    void setAppState(PR_SCOPE_KEY, parsed).catch(() => undefined);
  }

  const counts = createMemo(() => scopeCounts(state().pull_requests));

  /* Depends on the counts and nothing else, so a click on the control does not
     hand it a fresh set of options. */
  const scopeOptions = createMemo(() =>
    PR_SCOPES.map((value) => ({ value, label: `${scopeLabel(value)} ${counts()[value]}` })),
  );

  const grouped = createMemo(() => {
    const workspace = forgeStore.workspaces.find((item) => item.id === activeWorkspace());
    return filterPullRequests(
      state().pull_requests,
      scope(),
      query(),
      workspace?.project_id ?? null,
    );
  });

  /** The pull request whose tab is on screen, so its card reads as selected. */
  const openKey = createMemo(() => {
    const active = currentViews().active;
    return active.kind === "pr_detail" || active.kind === "pr_review" ? active.key : null;
  });

  function refresh(): void {
    setLoading("pull_requests", true);
    void refreshPullRequests().catch(() => undefined);
  }

  function card(pr: PullRequest) {
    const key = prKey(pr);
    const selected = () => openKey() === key;
    const decision = () => decisionChip(pr.review_decision);
    return (
      <ListCard
        class="pr-card"
        selected={selected()}
        ident={`#${pr.number}`}
        openLabel={`Open pull request #${pr.number}: ${pr.title}`}
        onOpen={() => openPrDetail(key)}
        title={pr.title}
        aside={
          <Show when={pr.is_draft}>
            <Badge>draft</Badge>
          </Show>
        }
        /* Below the card and not in its header: the header is a button, and a
           button inside a button is not a control a keyboard can reach. */
        actions={
          <Button
            variant={selected() ? "primary" : "secondary"}
            size="xs"
            aria-label={`Review pull request #${pr.number} with an agent, read-only`}
            onClick={() => openPrReview(key)}
          >
            Review
          </Button>
        }
        meta={
          <>
            <span class="pr-card-branches">
              {pr.head_ref} → {pr.base_ref}
            </span>
            <span>{pr.author}</span>
            <Show when={decision()}>
              {(chip) => (
                <span class="pr-decision" data-tone={chip().tone}>
                  {chip().label}
                </span>
              )}
            </Show>
          </>
        }
      />
    );
  }

  return (
    <div class="panel-body pr-panel">
      <div class="pr-panel-head">
        <h2 class="forge-section-label">Pull requests</h2>
        <span class="history-spacer" />
        <Show when={state().refreshed_at}>
          {(at) => <span class="pr-synced">synced {new Date(at()).toLocaleTimeString()}</span>}
        </Show>
        <IconButton
          label="Refresh pull requests from the host"
          size="xs"
          variant="ghost"
          loading={loading.pull_requests}
          onClick={refresh}
        >
          <Icon name="refresh" size={13} />
        </IconButton>
      </div>

      <SearchField
        class="panel-search"
        size="sm"
        aria-label="Filter pull requests"
        placeholder="Filter pull requests…"
        value={query()}
        onChange={setQuery}
        onClear={() => setQuery("")}
      />
      <RadioGroup
        label="Which pull requests"
        variant="segmented"
        class="pr-scope"
        itemClass="pr-scope-item"
        value={scope()}
        onChange={pickScope}
        options={scopeOptions()}
      />

      {/* A per-host failure that did not invalidate everything: the rest of the
          list is still real, so it is shown with the gap named. The daemon's
          own text is the tooltip; `failureCopy` writes the sentence. */}
      <For each={state().failures}>
        {(failure) => (
          <p class="panel-error" title={failure.message}>
            {failureCopy(failure)}
          </p>
        )}
      </For>
      {/* Only when nothing was classified: the daemon composes `error` out of
          the same host messages, so showing both said it twice. */}
      <Show when={state().failures.length === 0 && state().error}>
        {(error) => <p class="panel-error">{error()}</p>}
      </Show>

      <Show
        when={grouped().mine.length + grouped().others.length > 0}
        fallback={<p class="empty-copy">{scopeEmptyCopy(scope())}</p>}
      >
        <div class="pr-cards">
          <For each={grouped().mine}>{card}</For>
        </div>
        <Show when={grouped().mine.length > 0 && grouped().others.length > 0}>
          <div class="pr-divider">
            <h3 class="forge-section-label">Others</h3>
          </div>
        </Show>
        <div class="pr-cards">
          <For each={grouped().others}>{card}</For>
        </div>
      </Show>
    </div>
  );
}
