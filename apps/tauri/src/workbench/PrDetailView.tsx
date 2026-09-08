import { For, Show, createMemo } from "solid-js";
import { relativeAge } from "../panels/featureRows";
import { openUrl } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";
import { openPrReview } from "../store/viewsStore";
import { Icon } from "../theme/icons";
import { Badge, Button, IconButton, toast } from "../ui";
import { Markdown } from "./Markdown";
import {
  commentSummary,
  decisionChip,
  fileSummary,
  labelColor,
  monogram,
  readableDate,
  relationChips,
  splitRepository,
  stateChip,
} from "./prDetail";

import type { PullRequest } from "../runtime/types";

export type PrDetailViewProps = {
  prKey: string;
};

/** Stable key for a pull request in the cache. */
export function prKey(pr: PullRequest): string {
  return `${pr.host}/${pr.repository}#${pr.number}`;
}

export function findPullRequest(key: string): PullRequest | null {
  return forgeStore.pull_requests.pull_requests.find((pr) => prKey(pr) === key) ?? null;
}

/**
 * One pull request opened as a centre tab: the description rendered, and the
 * facts of the row beside it.
 *
 * Everything on screen is the last refresh — this tab is forbidden from asking
 * the host anything, so a number here is as old as the panel's timestamp says.
 * The sidebar is deliberately only what the cached row carries: checks, review
 * threads and comments are not in it, and inventing tabs for them would be a
 * promise the daemon does not keep.
 */
export function PrDetailView(props: PrDetailViewProps) {
  const pr = createMemo(() => findPullRequest(props.prKey));

  function copyLink(url: string): void {
    void navigator.clipboard
      .writeText(url)
      .then(() => toast({ title: "Link copied", tone: "success" }))
      .catch(() => undefined);
  }

  return (
    <Show when={pr()} fallback={<p class="empty-copy">Pull request not found in the last read.</p>}>
      {(item) => (
        <div class="pr-detail">
          <header class="pr-detail-head">
            <div class="pr-detail-trail">
              <Icon name="git-pull-request" size={13} class="forge-icon-muted" />
              <Show when={splitRepository(item().repository).owner}>
                {(owner) => (
                  <>
                    <span class="pr-trail-owner">{owner()}</span>
                    <span class="pr-trail-sep" aria-hidden="true">
                      /
                    </span>
                  </>
                )}
              </Show>
              <span class="pr-trail-repo">{splitRepository(item().repository).name}</span>
              <span class="pr-trail-sep" aria-hidden="true">
                ·
              </span>
              <span class="pr-trail-number">#{item().number}</span>
              <div class="pr-detail-tools">
                <IconButton label="Copy link" onClick={() => copyLink(item().url)}>
                  <Icon name="copy" size={14} />
                </IconButton>
                <IconButton
                  label="Open in browser"
                  onClick={() => void openUrl(item().url).catch(() => undefined)}
                >
                  <Icon name="external-link" size={14} />
                </IconButton>
              </div>
            </div>

            <div class="pr-detail-title-row">
              <h2 class="pr-detail-title">
                {item().title} <span class="pr-detail-number">#{item().number}</span>
              </h2>
              <Button
                variant="primary"
                iconLeading={<Icon name="agent" size={14} />}
                onClick={() => openPrReview(props.prKey)}
              >
                Review with an agent
              </Button>
            </div>

            <div class="pr-detail-facts">
              <Badge tone={stateChip(item()).tone} class="pr-state">
                <Icon name="git-pull-request" size={11} />
                {stateChip(item()).label}
              </Badge>
              <span class="pr-fact-author">{item().author}</span>
              <span class="pr-fact-branches">
                <code>{item().head_ref}</code>
                <span aria-hidden="true">→</span>
                <code>{item().base_ref}</code>
              </span>
              <span class="pr-fact-time">
                updated {relativeAge(item().updated_at, new Date())} ago
              </span>
              <For each={relationChips(item().relations)}>
                {(chip) => <Badge tone={chip.tone}>{chip.label}</Badge>}
              </For>
            </div>
          </header>

          <div class="pr-detail-main">
            <article class="pr-detail-body">
              <Show when={item().body.trim()} fallback={<p class="empty-copy">No description.</p>}>
                <Markdown text={item().body} />
              </Show>
              <Show when={item().body_truncated}>
                <p class="pr-detail-note">
                  The host's description was longer than the read allows, so the end of it is not
                  here. Open it in the browser for the rest.
                </p>
              </Show>
            </article>

            <aside class="pr-detail-side" aria-label="Pull request facts">
              <section class="pr-side-section">
                <h3 class="pr-side-title">Review</h3>
                <Show
                  when={decisionChip(item().review_decision)}
                  fallback={<p class="pr-side-empty">No verdict yet.</p>}
                >
                  {(chip) => <Badge tone={chip().tone}>{chip().label}</Badge>}
                </Show>
              </section>
              <People
                title="Reviewers"
                names={item().review_requests}
                empty="No reviewers requested."
              />
              <People title="Assignees" names={item().assignees} empty="Nobody assigned." />
              <Show when={item().labels.length > 0}>
                <section class="pr-side-section">
                  <h3 class="pr-side-title">Labels</h3>
                  <ul class="pr-side-labels">
                    <For each={item().labels}>
                      {(label) => (
                        <li class="pr-label">
                          <span
                            class="pr-label-dot"
                            aria-hidden="true"
                            style={
                              labelColor(label.color) === null
                                ? undefined
                                : { background: labelColor(label.color) ?? "" }
                            }
                          />
                          {label.name}
                        </li>
                      )}
                    </For>
                  </ul>
                </section>
              </Show>
              <section class="pr-side-section">
                <h3 class="pr-side-title">Changes</h3>
                <p class="pr-side-line">
                  <span class="pr-added">+{item().additions}</span>{" "}
                  <span class="pr-removed">−{item().deletions}</span>
                </p>
                <p class="pr-side-empty">{fileSummary(item().changed_files)}</p>
              </section>
              <section class="pr-side-section">
                <h3 class="pr-side-title">Activity</h3>
                <p class="pr-side-line">{commentSummary(item().comment_count)}</p>
                <p class="pr-side-empty">Opened {readableDate(item().created_at)}</p>
              </section>
            </aside>
          </div>
        </div>
      )}
    </Show>
  );
}

/** A named list in the sidebar, with a monogram where a forge shows a face. */
function People(props: { title: string; names: string[]; empty: string }) {
  return (
    <section class="pr-side-section">
      <h3 class="pr-side-title">{props.title}</h3>
      <Show when={props.names.length > 0} fallback={<p class="pr-side-empty">{props.empty}</p>}>
        <ul class="pr-side-people">
          <For each={props.names}>
            {(name) => (
              <li class="pr-person">
                <span class="pr-person-mark" aria-hidden="true">
                  {monogram(name)}
                </span>
                {name}
              </li>
            )}
          </For>
        </ul>
      </Show>
    </section>
  );
}
