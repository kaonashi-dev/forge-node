import { For, Show, createMemo, createSignal } from "solid-js";
import { relativeAge } from "./relativeAge";
import { openUrl } from "../../runtime/host";
import { PathText } from "../files/references/PathText";
import { forgeStore } from "../../state/forgeStore";
import { focus, openPrReview, showTerminal } from "../../navigation/viewsStore";
import { TERMINAL_VIEW } from "../../navigation/views";
import { selectSession } from "../sessions/commands";
import { reviewRun } from "./prReviewStore";
import { Icon } from "../../theme/icons/index";
import { Badge, Button, IconButton, Tabs, toast } from "../../ui/index";
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
          <header class="pr-detail-head">
            <div class="pr-detail-trail">
              <span class="pr-state" data-state={item().is_draft ? "draft" : "open"}>
                <Icon name="git-pull-request" size={12} />
                {stateChip(item()).label}
              </span>
              <span class="pr-detail-repo">
                {item().repository} #{item().number}
              </span>
              <div class="pr-detail-tools">
                <IconButton label="Copy link" onClick={() => copyLink(item().url)}>
                  <Icon name="copy" size={14} />
                </IconButton>
                <IconButton
                  label="Open on the host"
                  onClick={() => void openUrl(item().url).catch(() => undefined)}
                >
                  <Icon name="external-link" size={14} />
                </IconButton>
              </div>
            </div>

            <div class="pr-detail-title-row">
              <h2 class="pr-detail-title">{item().title}</h2>
              <Button
                variant="primary"
                iconLeading={<Icon name="agent" size={14} />}
                onClick={() => openPrReview(props.prKey)}
              >
                Review with an agent
              </Button>
            </div>

            <div class="pr-detail-facts">
              <span class="pr-fact-branches">
                <code>{item().head_ref}</code>
                <span aria-hidden="true">→</span>
                <span class="forge-visually-hidden">into</span>
                <code>{item().base_ref}</code>
              </span>
              <span class="pr-fact-sep" aria-hidden="true" />
              <span>{fileSummary(item().changed_files)}</span>
              <span class="pr-added">+{item().additions}</span>
              <span class="pr-removed">−{item().deletions}</span>
              <span class="pr-fact-sep" aria-hidden="true" />
              <span>
                {item().author} · updated {relativeAge(item().updated_at, new Date())} ago
              </span>
              <For each={relationChips(item().relations)}>
                {(chip) => <Badge tone={chip.tone}>{chip.label}</Badge>}
              </For>
            </div>
          </header>

          <div class="pr-detail-main">
            <Tabs
              class="pr-detail-tabs"
              listClass="pr-detail-tab-list"
              triggerClass="forge-tab"
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
            <ReviewRail prKey={props.prKey} url={item().url} onCopy={() => copyLink(item().url)} />
          </div>
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
          fallback={<p class="pr-side-empty">No verdict yet.</p>}
        >
          {(chip) => <Badge tone={chip().tone}>{chip().label}</Badge>}
        </Show>
      </section>
      <People title="Reviewers" names={props.pr.review_requests} empty="No reviewers requested." />
      <People title="Assignees" names={props.pr.assignees} empty="Nobody assigned." />
      <Show when={props.pr.labels.length > 0}>
        <section class="pr-side-section">
          <h3 class="forge-section-label">Labels</h3>
          <ul class="pr-side-labels">
            <For each={props.pr.labels}>
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
        <h3 class="forge-section-label">Activity</h3>
        <p class="pr-side-line">{commentSummary(props.pr.comment_count)}</p>
        <p class="pr-side-empty">Opened {readableDate(props.pr.created_at)}</p>
      </section>
    </div>
  );
}

/**
 * Where the agent's pass stands. Its findings are prose in its own session,
 * never posted: what reaches the pull request is the reader's call.
 */
function ReviewRail(props: { prKey: string; url: string; onCopy: () => void }) {
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
    <aside class="pr-review-rail" aria-labelledby="pr-review-rail-title">
      <div class="pr-divider">
        <h3 id="pr-review-rail-title" class="forge-section-label">
          Agent review
        </h3>
      </div>
      <div
        class="pr-review-status"
        data-state={session() ? "running" : run() ? "starting" : "idle"}
      >
        <span class="pr-review-status-mark" aria-hidden="true" />
        <span class="pr-review-status-text">
          <Show when={session()} fallback={run() ? "Starting the agent…" : "No agent pass yet."}>
            {(current) => `${sessionTitle(current())} · ${sessionStateLabel(current().state)}`}
          </Show>
        </span>
      </div>
      <Show
        when={run()}
        fallback={
          <Button variant="secondary" size="sm" onClick={() => openPrReview(props.prKey)}>
            Choose what to ask for
          </Button>
        }
      >
        <Button variant="secondary" size="sm" disabled={!session()} onClick={showSession}>
          Read the findings
        </Button>
      </Show>
      <span class="history-spacer" />
      <div class="pr-review-rail-foot">
        <p class="pr-review-rail-note">
          The agent reads the pull request and never writes to it. Posting is yours.
        </p>
        <div class="pr-review-rail-actions">
          <Button variant="secondary" size="sm" onClick={props.onCopy}>
            Copy link
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void openUrl(props.url).catch(() => undefined)}
          >
            Open on the host
          </Button>
        </div>
      </div>
    </aside>
  );
}

/** A named list, with a monogram where a forge shows a face. */
function People(props: { title: string; names: string[]; empty: string }) {
  return (
    <section class="pr-side-section">
      <h3 class="forge-section-label">{props.title}</h3>
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
