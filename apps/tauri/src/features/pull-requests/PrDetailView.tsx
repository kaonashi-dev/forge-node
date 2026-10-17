import { For, Show, createMemo, createSignal } from "solid-js";
import { relativeAge } from "./relativeAge";
import { openUrl } from "../../runtime/host";
import { PathText } from "../files/references/PathText";
import { forgeStore } from "../../state/forgeStore";
import { focus, openPrReview, showTerminal } from "../../navigation/viewsStore";
import { TERMINAL_VIEW } from "../../navigation/views";
import { selectSession } from "../sessions/commands";
import { reviewRun } from "./prReviewStore";
import { Badge, Button, Tabs, toast } from "../../ui/index";
import { Markdown } from "../../shared/markdown/Markdown";
import {
  commentSummary,
  decisionChip,
  fileSummary,
  labelColor,
  monogram,
  readableDate,
  relationChips,
  stateChip,
} from "./prDetail";

import {
  sessionStateLabel,
  sessionTitle,
  type PullRequest,
  type Session,
} from "../../contracts/runtime";

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
 * One pull request opened as a centre tab, read from the last refresh.
 *
 * This tab is forbidden from asking the host anything, so a number here is as
 * old as the panel's timestamp says. Checks, review threads and the diff are
 * not in the cached row, and a tab for them would be a promise the daemon does
 * not keep.
 */
export function PrDetailView(props: PrDetailViewProps) {
  const pr = createMemo(() => findPullRequest(props.prKey));
  const [tab, setTab] = createSignal("description");

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
          <div class="pr-detail-main">
            <header class="pr-detail-head">
              <div class="pr-detail-trail">
                <Badge class="pr-state" tone={stateChip(item()).tone}>
                  {stateChip(item()).label}
                </Badge>
                <span class="pr-detail-repo">
                  {item().repository} #{item().number}
                </span>
                <For each={relationChips(item().relations)}>
                  {(chip) => <Badge tone={chip.tone}>{chip.label}</Badge>}
                </For>
                <div class="pr-detail-tools">
                  <Button variant="ghost" size="md" onClick={() => copyLink(item().url)}>
                    Copy link
                  </Button>
                  <Button
                    variant="ghost"
                    size="md"
                    onClick={() => void openUrl(item().url).catch(() => undefined)}
                  >
                    Open on the host
                  </Button>
                </div>
              </div>

              <h1 class="pr-detail-title">{item().title}</h1>

              <div class="pr-detail-facts">
                <span class="pr-fact-branches">
                  <code>{item().head_ref}</code>
                  <span class="pr-fact-arrow" aria-hidden="true">
                    →
                  </span>
                  <span class="forge-visually-hidden">into</span>
                  <code>{item().base_ref}</code>
                </span>
                <span>{fileSummary(item().changed_files)}</span>
                <span class="pr-fact-counts">
                  <span class="pr-added">+{item().additions}</span>{" "}
                  <span class="pr-removed">−{item().deletions}</span>
                </span>
                <span>
                  {item().author} · updated {relativeAge(item().updated_at, new Date())} ago
                </span>
              </div>

              <div class="pr-detail-actions">
                <Button variant="primary" size="lg" onClick={() => openPrReview(props.prKey)}>
                  Review with an agent
                </Button>
              </div>
            </header>

            <Tabs
              variant="underline"
              class="pr-detail-tabs"
              contentClass="pr-detail-panel"
              aria-label="Pull request sections"
              value={tab()}
              onChange={setTab}
              tabs={[
                {
                  value: "description",
                  label: "Description",
                  content: () => (
                    <article class="pr-detail-body">
                      <Show
                        when={item().body.trim()}
                        fallback={<p class="empty-copy">No description.</p>}
                      >
                        <Markdown
                          text={item().body}
                          onOpenUrl={(href) => void openUrl(href).catch(() => undefined)}
                          renderPath={(text) => <PathText text={text} />}
                        />
                      </Show>
                      <Show when={item().body_truncated}>
                        <p class="pr-detail-note">
                          The host's description was longer than the read allows, so the end of it
                          is not here. Open it on the host for the rest.
                        </p>
                      </Show>
                    </article>
                  ),
                },
                {
                  value: "details",
                  label: "Details",
                  content: () => <Details pr={item()} />,
                },
              ]}
            />
          </div>
          <aside class="pr-detail-rail" aria-label="Pull request facts">
            <section class="pr-side-section">
              <h2 class="forge-section-label">Verdict</h2>
              <Show
                when={decisionChip(item().review_decision)}
                fallback={<p class="pr-side-empty">No verdict yet.</p>}
              >
                {(chip) => (
                  <p class="pr-side-line pr-verdict" data-tone={chip().tone}>
                    {chip().label}
                  </p>
                )}
              </Show>
            </section>
            <People title="Reviewers" names={item().review_requests} empty="None requested" />
            <section class="pr-side-section">
              <h2 class="forge-section-label">Labels</h2>
              <Show when={item().labels.length > 0} fallback={<p class="pr-side-empty">None</p>}>
                <Labels labels={item().labels} />
              </Show>
            </section>
            <ReviewRail prKey={props.prKey} />
          </aside>
        </div>
      )}
    </Show>
  );
}

function Details(props: { pr: PullRequest }) {
  return (
    <div class="pr-detail-facts-grid">
      <section class="pr-side-section">
        <h3 class="forge-section-label">Verdict</h3>
        <Show
          when={decisionChip(props.pr.review_decision)}
          fallback={<p class="forge-caption pr-side-empty">No verdict yet.</p>}
        >
          {(chip) => <Badge tone={chip().tone}>{chip().label}</Badge>}
        </Show>
      </section>
      <People title="Reviewers" names={props.pr.review_requests} empty="No reviewers requested." />
      <People title="Assignees" names={props.pr.assignees} empty="Nobody assigned." />
      <Show when={props.pr.labels.length > 0}>
        <section class="pr-side-section">
          <h3 class="forge-section-label">Labels</h3>
          <Labels labels={props.pr.labels} />
        </section>
      </Show>
      <section class="pr-side-section">
        <h3 class="forge-section-label">Activity</h3>
        <p class="pr-side-line">{commentSummary(props.pr.comment_count)}</p>
        <p class="forge-caption pr-side-empty">Opened {readableDate(props.pr.created_at)}</p>
      </section>
    </div>
  );
}

function Labels(props: { labels: PullRequest["labels"] }) {
  return (
    <ul class="pr-side-labels">
      <For each={props.labels}>
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
  );
}

/**
 * Where the agent's pass stands. Its findings are prose in its own session,
 * never posted: what reaches the pull request is the reader's call.
 */
function ReviewRail(props: { prKey: string }) {
  const run = () => reviewRun(props.prKey);
  const session = createMemo((): Session | null => {
    const id = run()?.session ?? null;
    return id ? (forgeStore.sessions.find((item) => item.id === id) ?? null) : null;
  });

  function showSession(): void {
    const id = run()?.session ?? null;
    if (id) void selectSession(id).catch(() => undefined);
    showTerminal();
    focus(TERMINAL_VIEW);
  }

  return (
    <section class="pr-review-card" aria-labelledby="pr-review-rail-title">
      <h2 id="pr-review-rail-title" class="pr-review-card-title">
        Agent review
      </h2>
      <p class="pr-review-card-note">
        Runs read-only in this checkout. It can't edit, commit or post; you decide what reaches the
        host.
      </p>
      <p class="pr-review-status">
        <span
          class="pr-review-status-mark"
          data-state={session() ? "running" : run() ? "starting" : "idle"}
          aria-hidden="true"
        />
        <Show when={session()} fallback={run() ? "Starting the agent…" : "Not started"}>
          {(current) => `${sessionTitle(current())} · ${sessionStateLabel(current().state)}`}
        </Show>
      </p>
      <Show when={run()}>
        <Button variant="secondary" size="sm" disabled={!session()} onClick={showSession}>
          Read the findings
        </Button>
      </Show>
    </section>
  );
}

/** A named list, with a monogram where a forge shows a face. */
function People(props: { title: string; names: string[]; empty: string }) {
  return (
    <section class="pr-side-section">
      <h3 class="forge-section-label">{props.title}</h3>
      <Show
        when={props.names.length > 0}
        fallback={<p class="forge-caption pr-side-empty">{props.empty}</p>}
      >
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
