import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { Finding } from "../../contracts/workbench";
import { openUrl } from "../../runtime/host";
import { Markdown } from "../../shared/markdown/Markdown";
import { forgeStore } from "../../state/forgeStore";
import { Badge, Button, EmptyState, Progress } from "../../ui/index";
import { openEditorAt } from "../editor/open";
import { openFile } from "../files/commands";
import { PathText } from "../files/references/PathText";
import { cancelReview, listReviews, startReview } from "./commands";
import {
  duration,
  groupFindings,
  isStale,
  scoreLabel,
  severityTone,
  stageLabel,
  stageProgress,
  verdictLabel,
  verdictTone,
  type Grouping,
} from "./reviewDisplay";
import { reviewStore } from "./state";

/** Opening the tab reads the latest run; it never starts one. */
export function ReviewView(props: {
  workspace: string;
  onFocusSession: (session: string) => void;
}) {
  const [grouping, setGrouping] = createSignal<Grouping>("severity");
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(timer));

  createEffect(() => void listReviews(props.workspace).catch(() => undefined));

  const run = () => reviewStore.latest[props.workspace];
  const report = () => {
    const current = reviewStore.reports[props.workspace];
    return current && current.run.id === run()?.id ? current : undefined;
  };
  const running = () => {
    const status = run()?.status;
    return status === "Queued" || status === "Running";
  };
  const head = () =>
    forgeStore.workspaces.find((workspace) => workspace.id === props.workspace)?.status.head;
  const groups = createMemo(() => groupFindings(report()?.findings ?? [], grouping()));

  function openAt(finding: Finding): void {
    openEditorAt(finding.path, finding.line ?? 1);
    void openFile(props.workspace, finding.path).catch(() => undefined);
  }

  const start = () => void startReview(props.workspace).catch(() => undefined);

  return (
    <div class="diff-view">
      <Show when={reviewStore.errors[props.workspace]}>
        {(error) => <p class="panel-error">{error()}</p>}
      </Show>
      <Show
        when={run()}
        fallback={
          <section class="review-section">
            <EmptyState message="No review of this workspace yet." />
            <Button variant="primary" size="sm" onClick={start}>
              Review changes
            </Button>
          </section>
        }
      >
        {(current) => (
          <>
            <header class="diff-header review-header">
              <Show
                when={current().verdict}
                fallback={<Badge tone="neutral">{current().status}</Badge>}
              >
                {(verdict) => (
                  <Badge tone={verdictTone(verdict())}>{verdictLabel(verdict())}</Badge>
                )}
              </Show>
              <span>{scoreLabel(current().score)}</span>
              <span class="panel-note">{current().tier}</span>
              <span class="panel-note">{current().preset}</span>
              <span class="session-changes-base">{current().head_oid.slice(0, 7)}</span>
              <span class="panel-note">{current().provider}</span>
              <span class="panel-note">{duration(current(), now())}</span>
              <Show when={isStale(current(), head())}>
                <Badge tone="attention">stale</Badge>
              </Show>
              <span class="history-spacer" />
              <Show
                when={running()}
                fallback={
                  <Button variant="secondary" size="xs" onClick={start}>
                    Review again
                  </Button>
                }
              >
                <Button
                  variant="secondary"
                  size="xs"
                  onClick={() => void cancelReview(current().id).catch(() => undefined)}
                >
                  Cancel
                </Button>
              </Show>
            </header>

            <Show when={running()}>
              <section class="review-section">
                <Progress
                  label={stageLabel(current().stage)}
                  value={stageProgress(current().stage)}
                  detail={duration(current(), now())}
                />
                <Show when={current().session_id}>
                  {(session) => (
                    <Button
                      variant="ghost"
                      size="xs"
                      onClick={() => props.onFocusSession(session())}
                    >
                      Show the reviewer session
                    </Button>
                  )}
                </Show>
              </section>
            </Show>

            <Show when={current().error}>{(error) => <p class="panel-error">{error()}</p>}</Show>

            <Show when={current().summary}>
              <section class="review-section">
                <h4 class="forge-section-label panel-subhead">Summary</h4>
                <Markdown
                  text={current().summary}
                  onOpenUrl={(href) => void openUrl(href).catch(() => undefined)}
                  renderPath={(text) => <PathText text={text} />}
                />
              </section>
            </Show>

            <Show when={report()}>
              {(loaded) => (
                <section class="review-section">
                  <header class="forge-row">
                    <h4 class="forge-section-label panel-subhead">
                      {loaded().findings.length} finding{loaded().findings.length === 1 ? "" : "s"}
                    </h4>
                    <span class="history-spacer" />
                    <Button
                      variant="secondary"
                      size="xs"
                      onClick={() => setGrouping(grouping() === "severity" ? "file" : "severity")}
                    >
                      {grouping() === "severity" ? "By severity" : "By file"}
                    </Button>
                  </header>
                  <For each={groups()}>
                    {(group) => (
                      <>
                        <h5 class="forge-section-label panel-subhead">{group.key}</h5>
                        <For each={group.findings}>
                          {(finding) => (
                            <article class="review-finding">
                              <div class="forge-row review-finding-head">
                                <Badge tone={severityTone(finding.severity)}>
                                  {finding.severity}
                                </Badge>
                                <span class="panel-note">{finding.category}</span>
                                <Show when={finding.rule_id}>
                                  {(rule) => <span class="panel-note">rule: {rule()}</span>}
                                </Show>
                                <span class="panel-note">
                                  {Math.round(finding.confidence * 100)}%
                                </span>
                                <span class="review-finding-title forge-truncate">
                                  {finding.title}
                                </span>
                                <span class="history-spacer" />
                                <Button variant="ghost" size="xs" onClick={() => openAt(finding)}>
                                  {finding.line === null
                                    ? finding.path
                                    : `${finding.path}:${finding.line}`}
                                </Button>
                              </div>
                              <p class="review-finding-body">{finding.body}</p>
                              <Show when={finding.suggestion}>
                                {(suggestion) => (
                                  <pre class="review-finding-suggestion">{suggestion()}</pre>
                                )}
                              </Show>
                            </article>
                          )}
                        </For>
                      </>
                    )}
                  </For>
                </section>
              )}
            </Show>

            <section class="review-section">
              <h4 class="forge-section-label panel-subhead">Coverage</h4>
              <p class="panel-note">
                {current().coverage.included.length} included · {current().coverage.excluded.length}{" "}
                excluded · {current().coverage.truncated.length} truncated ·{" "}
                {current().coverage.redactions} redacted
              </p>
              <For each={current().coverage.notes}>
                {(note) => <p class="panel-note">{note}</p>}
              </For>
            </section>
          </>
        )}
      </Show>
    </div>
  );
}
