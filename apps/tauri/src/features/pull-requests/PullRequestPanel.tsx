import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import { setAppState } from "../settings/commands";
import { forgeStore } from "../../state/forgeStore";
import { currentViews, openPrDetail } from "../../navigation/viewsStore";
import { prKey } from "./PrDetailView";
import {
  PR_SCOPES,
  PR_SCOPE_KEY,
  filterPullRequests,
  parseScope,
  failureCopy,
  scopeEmptyCopy,
  scopeLabel,
  type PrScope,
} from "./prFilters";
import { decisionChip } from "./prDetail";
import { Icon } from "../../theme/icons/index";
import { Badge, IconButton, RadioGroup, SearchField } from "../../ui/index";
import { refreshPullRequests } from "./commands";
import { loading, setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";
import type { PullRequest } from "../../contracts/runtime";

// Snapshot data is cached; remote refreshes complete through events.
/** `owner/hellopay-backend` reads as the repository the person knows. */
function repositoryName(repository: string): string {
  return repository.slice(repository.lastIndexOf("/") + 1);
}

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

  const scopeOptions = PR_SCOPES.map((value) => ({ value, label: scopeLabel(value) }));

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
    /* A review that is waiting on *you* is the one case the list says in ember. */
    const tone = () => {
      const chip = decision();
      if (!chip) return null;
      return chip.tone === "warn" && pr.relations.review_requested ? "attention" : chip.tone;
    };
    return (
      <button
        type="button"
        class="forge-row pr-row"
        aria-current={selected() ? "true" : undefined}
        aria-label={`Open pull request #${pr.number}: ${pr.title}`}
        onClick={() => openPrDetail(key)}
      >
        <span class="pr-row-line">
          <span class="pr-row-number">#{pr.number}</span>
          <span class="pr-row-title">{pr.title}</span>
        </span>
        <span class="pr-row-meta">
          <span class="pr-row-repo forge-truncate">{repositoryName(pr.repository)}</span>
          <Show when={!pr.is_draft} fallback={<Badge>Draft</Badge>}>
            <Show when={decision()}>
              {(chip) => (
                <span class="pr-decision" data-tone={tone()}>
                  {chip().label}
                </span>
              )}
            </Show>
          </Show>
        </span>
      </button>
    );
  }

  return (
    <div class="panel-body pr-panel">
      <div class="pr-panel-head">
        <h2 class="pr-panel-title">Pull requests</h2>
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
          <Icon name="refresh" size={13} class="forge-icon-muted" />
        </IconButton>
      </div>

      <SearchField
        class="forge-filter-field"
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
        options={scopeOptions}
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
