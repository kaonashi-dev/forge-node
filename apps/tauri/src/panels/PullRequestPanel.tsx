import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import { setAppState } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";
import { setLoading, workbenchStore } from "../store/workbenchStore";
import { refreshPullRequests } from "../workbench/api";
import { openPrDetail, openPrReview } from "../store/viewsStore";
import { prKey } from "../workbench/PrDetailView";
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
import { Badge, Button, FilterHeader, ListCard, Tooltip } from "../ui";

/**
 * Open pull requests, from the daemon's cache.
 *
 * `GetSnapshot` is forbidden from touching the network, so what is on screen is
 * the last read — and refreshing is a deliberate act. The daemon coalesces the
 * refresh globally (one covers every repository) and answers with an event, so
 * this asks and then waits like everything else.
 *
 * The scope chips cost nothing: the daemon asked the host all three viewer
 * questions in the one GraphQL query it already runs, and ships the answers on
 * every row (§16.6).
 */
export function PullRequestPanel() {
  const [scope, setScope] = createSignal<PrScope>("all");
  const [query, setQuery] = createSignal("");

  const state = () => forgeStore.pull_requests;

  onMount(() => setScope(parseScope(forgeStore.app_state[PR_SCOPE_KEY])));

  function pickScope(next: PrScope): void {
    setScope(next);
    void setAppState(PR_SCOPE_KEY, next).catch(() => undefined);
  }

  const counts = createMemo(() => scopeCounts(state().pull_requests));

  const grouped = createMemo(() => {
    const workspace = forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace);
    return filterPullRequests(
      state().pull_requests,
      scope(),
      query(),
      workspace?.project_id ?? null,
    );
  });

  function refresh(): void {
    setLoading("pull_requests", true);
    void refreshPullRequests().catch(() => undefined);
  }

  return (
    <div class="panel-body">
      <div class="panel-heading">
        Pull requests
        <Show when={state().refreshed_at}>
          {(at) => <span class="panel-note-inline">{new Date(at()).toLocaleTimeString()}</span>}
        </Show>
      </div>
      <FilterHeader
        label="Filter pull requests"
        placeholder="Filter pull requests…"
        query={query()}
        onQuery={setQuery}
        rows={[
          {
            label: "Which pull requests",
            value: scope(),
            onChange: pickScope,
            options: PR_SCOPES.map((value) => ({
              value,
              label: `${scopeLabel(value)} ${counts()[value]}`,
            })),
          },
        ]}
      />
      {/* A per-host failure that did not invalidate everything: the rest of the
          list is still real, so it is shown with the gap named. The daemon's
          own text is the tooltip, not the sentence — `failureCopy` writes the
          sentence from the classified kind. */}
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

      <For
        each={[...grouped().mine, ...grouped().others]}
        fallback={<p class="empty-copy">{scopeEmptyCopy(scope())}</p>}
      >
        {(pr) => (
          <ListCard
            class="pr-card"
            openLabel={`Open pull request #${pr.number}`}
            onOpen={() => openPrDetail(prKey(pr))}
            glyph={<span class="pr-number">#{pr.number}</span>}
            title={pr.title}
            aside={
              <Show when={pr.is_draft}>
                <Badge>draft</Badge>
              </Show>
            }
            /* Below the card and not in its header: the header *is* a button,
               and a button inside a button is not a control a keyboard can
               reach. */
            actions={
              <Tooltip label="Start an agent review, read-only" contents>
                <Button
                  variant="secondary"
                  size="xs"
                  aria-label={`Review pull request #${pr.number} with an agent`}
                  onClick={() => openPrReview(prKey(pr))}
                >
                  Review
                </Button>
              </Tooltip>
            }
            meta={
              <>
                <span>{pr.repository}</span>
                <span>{pr.author}</span>
                <span>
                  {pr.head_ref} → {pr.base_ref}
                </span>
                <Show when={pr.review_decision}>
                  {(decision) => <span class="pr-decision">{decision()}</span>}
                </Show>
              </>
            }
          />
        )}
      </For>

      <Button variant="secondary" onClick={refresh}>
        Refresh from the host
      </Button>
    </div>
  );
}
