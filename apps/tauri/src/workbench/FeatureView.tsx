import { For, Index, Show, createEffect, createMemo, createSignal, on } from "solid-js";
import * as harness from "../harness/api";
import {
  displayedFeature,
  featureLabel,
  hasArtifacts,
  runnableStep,
  statusLabel,
  type HarnessArtifactKind,
  type HarnessFeature,
} from "../harness/types";
import { GATE_DOCS, RUN_DOCS, docPath, missingDocNote, type DocTab } from "../harness/docs";
import { agentRuns, eventSummary, runDuration, runTone } from "../harness/progress";
import { FeatureProgress } from "../harness/FeatureProgress";
import { jobPreviewLine } from "../harness/stream";
import { selectSession } from "../runtime/api";
import { jobIsRunning } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { featureJobs, harnessStore, openFeature, setHarnessStore } from "../store/harnessStore";
import { close, openDiff } from "../store/viewsStore";
import { harnessFeatureAgents, harnessIsolationNote } from "../shell/sessionTree";
import { Icon, SessionGlyph, StateMarker } from "../theme/icons";
import { JobStreamView, RecordedStreamView } from "./JobStreamView";
import { PreviewTerminal } from "./PreviewTerminal";
import { Badge, Button, Progress, RadioGroup, TextField } from "../ui";

/**
 * One harness feature, as a centre tab.
 *
 * The whole cycle is here because the cycle is the thing: the gate that stops
 * it, the step that moves it, the agents doing the work, the documents they
 * wrote, and the stream of the run that is going. A panel could list features;
 * only a tab has room to *drive* one.
 *
 * Nothing here is authoritative. Every action goes to the daemon, and the
 * feature it answers with is what the tab redraws from — approving does not
 * optimistically flip a badge, because the daemon may refuse.
 */
export function FeatureView(props: { id: number }) {
  const project = () => harnessStore.project;
  const feature = () => displayedFeature(props.id, harnessStore.detail, harnessStore.features);

  // The artefact listener and the advance handler key off `openFeature`, not
  // the tab id. Focusing a parked feature tab used to leave that pointer on
  // the previous id, so the gate docs spun on `Reading…` and Approve updated
  // the list while this tab kept drawing the stale detail.
  createEffect(() => {
    openFeature(props.id);
  });

  // Re-read when the tab is opened on a feature the store has not fetched, and
  // whenever the tab is pointed at a different one.
  createEffect(() => {
    const id = project();
    if (!id) return;
    if (harnessStore.detail?.id === props.id || harnessStore.loadingDetail) return;
    void harness.loadFeatureDetail(id, props.id).catch(() => undefined);
  });

  const jobs = createMemo(() => featureJobs(props.id));
  const running = createMemo(() => jobs().some((job) => jobIsRunning(job.state)));
  const agents = createMemo(() =>
    harnessFeatureAgents(forgeStore.sessions, feature()?.orchestrator_session_id ?? null),
  );
  const isolation = createMemo(() =>
    harnessIsolationNote(
      forgeStore.sessions,
      forgeStore.workspaces,
      feature()?.orchestrator_session_id ?? null,
    ),
  );

  function refresh(): void {
    const id = project();
    if (id) void harness.loadFeatureDetail(id, props.id).catch(() => undefined);
  }

  return (
    <section class="feature-view" aria-label="Feature">
      <Show
        when={feature()}
        fallback={
          <div class="feature-view-body">
            <Show
              when={harnessStore.detailError}
              fallback={<p class="panel-note">Loading feature #{props.id}…</p>}
            >
              {(error) => (
                <>
                  <p class="panel-error">{error()}</p>
                  <div class="feature-actions">
                    <Button variant="secondary" onClick={refresh}>
                      Try again
                    </Button>
                  </div>
                </>
              )}
            </Show>
          </div>
        }
      >
        {(current) => (
          <>
            <header class="feature-view-head">
              <span class="feature-id">#{current().id}</span>
              <h2 class="feature-title">{featureLabel(current())}</h2>
              <span class="feature-status" classList={{ live: running() }}>
                <Show when={running()}>
                  <Icon
                    name="loader"
                    class="forge-icon-spin forge-icon-blue"
                    size={12}
                    title="Step running"
                  />
                </Show>
                {statusLabel(current().status)}
              </span>
              <span class="history-spacer" />
              <Show when={current().review_rounds}>
                {(rounds) => <span class="panel-note">{rounds()} review rounds</span>}
              </Show>
              <Button variant="secondary" size="xs" onClick={refresh}>
                Refresh
              </Button>
              <Button
                variant="secondary"
                size="xs"
                onClick={() => close({ kind: "feature", id: props.id })}
              >
                Close
              </Button>
            </header>

            <div class="feature-view-body">
              <FeatureProgress feature={current()} jobs={jobs()} />
              <Show when={isolation()}>{(note) => <p class="feature-isolation">{note()}</p>}</Show>

              <Show when={current().status === "blocked"}>
                <BlockedCard feature={current()} />
              </Show>

              <Show when={current().status === "spec_ready"}>
                <GateCard feature={current()} />
              </Show>

              <StepsCard feature={current()} />

              <AgentsCard rows={agents()} headless={jobs().length > 0} />

              <Show when={harnessStore.previewSession}>
                <PreviewTerminal />
              </Show>

              <ActionsRow feature={current()} running={running()} />

              <Show when={hasArtifacts(current())}>
                <DocsCard feature={current()} />
              </Show>

              <Timeline />

              <Show when={harnessStore.validate}>
                {(result) => (
                  <section
                    class="feature-validate"
                    classList={{ ok: result().ok, bad: !result().ok }}
                  >
                    <h3>{result().ok ? "Harness validates" : "Harness does not validate"}</h3>
                    <pre class="feature-validate-output">{result().output}</pre>
                  </section>
                )}
              </Show>

              <Show when={harnessStore.detailError}>
                {(error) => <p class="panel-error">{error()}</p>}
              </Show>
            </div>
          </>
        )}
      </Show>
    </section>
  );
}

/**
 * A feature the machine could not finish.
 *
 * `RetryStep` re-runs the step that died (and may resume the provider session);
 * `Reopen` throws the cycle back to `pending`. Both are human decisions —
 * nothing auto-retries past `max_step_attempts`.
 */
function BlockedCard(props: { feature: HarnessFeature }) {
  const project = () => harnessStore.project;

  function advance(action: Parameters<typeof harness.advance>[2]): void {
    const id = project();
    if (!id) return;
    const revision = harnessStore.detail?.revision ?? null;
    void harness.advance(id, props.feature.id, action, revision).catch(() => undefined);
  }

  const lastAttempt = () => props.feature.attempts?.at(-1) ?? null;
  const canResumeSession = () => Boolean(lastAttempt()?.provider_session_id);

  return (
    <section class="feature-blocked-card">
      <h3>Blocked — intervention needed</h3>
      <Show when={props.feature.blocked_reason}>
        {(reason) => <p class="feature-blocked">{reason()}</p>}
      </Show>
      <p class="panel-note">
        {canResumeSession()
          ? "Retry reuses the last provider session when the agent supports it, so research already paid for is not thrown away."
          : "Retry starts the failed step again. Reopen returns the feature to pending and clears the block."}
      </p>
      <div class="feature-actions">
        <Button
          variant="primary"
          loading={harnessStore.advancing}
          onClick={() => advance("RetryStep")}
        >
          {canResumeSession() ? "Retry step (resume session)" : "Retry step"}
        </Button>
        <Button
          variant="secondary"
          disabled={harnessStore.advancing}
          onClick={() => advance("Reopen")}
        >
          Reopen from pending
        </Button>
      </div>
    </section>
  );
}

/**
 * The human gate.
 *
 * The one moment the machine stops and waits, so it is the one card that shows
 * the thing being approved: the tabs read the three documents the decision
 * rests on, and a missing one names the file it looked for — "the gate file is
 * not there" is a fact about the run, not a caption.
 */
function GateCard(props: { feature: HarnessFeature }) {
  const [reason, setReason] = createSignal("");
  const project = () => harnessStore.project;

  function advance(action: Parameters<typeof harness.advance>[2]): void {
    const id = project();
    if (!id) return;
    // The row as this card drew it. Approving twice used to start the
    // implement step twice; now the second one is a no-op the daemon answers
    // with the current row, and the disabled button is only the first defence.
    const revision = harnessStore.detail?.revision ?? null;
    void harness.advance(id, props.feature.id, action, revision).catch(() => undefined);
  }

  return (
    <section class="feature-gate">
      <h3>Spec ready — human gate</h3>
      <p class="panel-note">
        Approving starts the implementation step on the daemon, so it continues even if this window
        goes away.
      </p>

      <DocReader feature={props.feature} tabs={GATE_DOCS} />

      <div class="feature-gate-actions">
        <Button
          variant="primary"
          loading={harnessStore.advancing}
          onClick={() => advance("ApproveSpec")}
        >
          Approve
        </Button>
        <Button
          variant="secondary"
          disabled={harnessStore.advancing}
          onClick={() => advance("ReviseSpec")}
        >
          Send back to spec
        </Button>
        <TextField
          class="panel-search feature-block-reason"
          aria-label="Reason to block"
          placeholder="Reason to block…"
          value={reason()}
          onChange={setReason}
        />
        <Button
          variant="danger"
          disabled={harnessStore.advancing}
          onClick={() => advance({ Block: { reason: reason().trim() || "Blocked from the GUI" } })}
        >
          Block
        </Button>
      </div>
    </section>
  );
}

/** The artefact reader outside the gate: same tabs, no decision attached. */
function DocsCard(props: { feature: HarnessFeature }) {
  return (
    <section class="feature-docs">
      <h3>Documents</h3>
      <DocReader feature={props.feature} tabs={RUN_DOCS} />
    </section>
  );
}

/**
 * A set of artefact tabs and the one that is open.
 *
 * Whole and scrollable, because a spec is read to be approved: a clipped
 * document hides the tasks under exactly the requirements somebody wanted to
 * check.
 */
function DocReader(props: { feature: HarnessFeature; tabs: DocTab[] }) {
  const project = () => harnessStore.project;
  const [selected, setSelected] = createSignal<HarnessArtifactKind>(props.tabs[0].kind);

  function show(next: HarnessArtifactKind): void {
    setSelected(next);
    const id = project();
    if (id) void harness.loadArtifact(id, props.feature.id, next).catch(() => undefined);
  }

  // Back to the first tab on a feature switch; a detail refresh of the same
  // feature must not yank the tab being read.
  createEffect(
    on(
      () => props.feature.id,
      () => setSelected(props.tabs[0].kind),
    ),
  );

  // The gate exists to be approved, so its first document loads with the card
  // rather than waiting for a click.
  createEffect(() => {
    const id = project();
    if (!id) return;
    if (
      harnessStore.artifact !== null ||
      harnessStore.loadingArtifact ||
      harnessStore.artifactError !== null
    )
      return;
    void harness.loadArtifact(id, props.feature.id, selected()).catch(() => undefined);
  });

  // An artefact the daemon could not find arrives as empty text, not as a
  // failure, so only non-empty text counts as the document being there.
  const text = createMemo(() => {
    const current = harnessStore.artifact;
    if (!current || current.kind !== selected()) return null;
    return current.text.trim().length > 0 ? current.text : null;
  });

  const missing = createMemo(() => {
    if (harnessStore.loadingArtifact || text() !== null) return null;
    return missingDocNote(
      docPath(selected(), props.feature.id, props.feature.slug),
      harnessStore.artifactError,
    );
  });

  return (
    <div class="feature-doc-reader">
      <RadioGroup
        label="Document"
        class="history-filters"
        orientation="horizontal"
        itemClass="forge-chip"
        value={selected()}
        onChange={(next) => show(next as HarnessArtifactKind)}
        options={props.tabs.map((tab) => ({
          value: tab.kind,
          label: tab.label,
          render: () => tab.label,
        }))}
      />
      <Show when={harnessStore.loadingArtifact}>
        <p class="panel-note">Reading…</p>
      </Show>
      {/* A missing artefact is the normal state of a step that has not run
          yet, so the path goes where the document would have been. */}
      <Show when={missing()}>{(note) => <p class="feature-doc-missing">{note()}</p>}</Show>
      <Show when={text()}>{(body) => <pre class="feature-artifact">{body()}</pre>}</Show>
    </div>
  );
}

function StepsCard(props: { feature: HarnessFeature }) {
  const jobs = createMemo(() => featureJobs(props.feature.id));
  const runs = createMemo(() => agentRuns(props.feature, jobs(), harnessStore.timeline));
  const open = () => harnessStore.openJob;

  return (
    <section class="feature-jobs">
      <h3>Agent executions · {runs().length}</h3>
      <Index
        each={runs()}
        fallback={
          <p class="panel-note">No executions recorded. Start the specification to begin.</p>
        }
      >
        {(run) => (
          <article class="harness-run" data-tone={runTone(run().state)}>
            <header class="harness-run-head">
              <Show
                when={run().state === "Running" || run().state === "Queued"}
                fallback={<Icon name="agent" size={16} />}
              >
                <Icon
                  name="loader"
                  class="forge-icon-spin forge-icon-blue"
                  size={16}
                  title={run().state}
                />
              </Show>
              <strong>{run().role}</strong>
              <span class="harness-run-provider">{run().provider}</span>
              <Show when={run().attempt !== null}>
                <span class="panel-note">Attempt {run().attempt}</span>
              </Show>
              <Badge tone={runTone(run().state)}>{run().state}</Badge>
            </header>
            <div class="harness-run-meta">
              <Show when={run().started}>
                <span>
                  Started{" "}
                  <time dateTime={run().started}>{new Date(run().started).toLocaleString()}</time>
                </span>
              </Show>
              <Show when={runDuration(run().started, run().ended)}>
                {(duration) => <span>Duration {duration()}</span>}
              </Show>
              <Show when={run().job?.exit_code !== null && run().job?.exit_code !== undefined}>
                <span>Exit {run().job?.exit_code}</span>
              </Show>
            </div>
            <Show
              when={
                run().detail ||
                jobPreviewLine(run().job?.last_line ?? null, run().job?.summary ?? "")
              }
            >
              {(note) => <p class="harness-run-note">{note()}</p>}
            </Show>
            <Show when={run().state === "Unconfirmed"}>
              <p class="panel-note">
                Attempt recorded; live status is unavailable. Refresh to reconcile.
              </p>
            </Show>
            <div class="feature-actions">
              <Show when={run().jobId}>
                {(id) => (
                  <Button
                    variant="secondary"
                    size="xs"
                    onClick={() => setHarnessStore("openJob", open() === id() ? null : id())}
                  >
                    {open() === id() ? "Hide output" : "View output"}
                  </Button>
                )}
              </Show>
              <Show when={jobIsRunning(run().job?.state ?? "") ? run().job : undefined}>
                {(job) => (
                  <Button
                    variant="danger-ghost"
                    size="xs"
                    onClick={() => void harness.cancelJob(job().id).catch(() => undefined)}
                  >
                    Stop
                  </Button>
                )}
              </Show>
            </div>
            <Show when={run().jobId && open() === run().jobId}>
              <Show
                when={run().job}
                fallback={<RecordedStreamView job={run().jobId!} label={run().role} />}
              >
                {(job) => <JobStreamView job={job()} />}
              </Show>
            </Show>
          </article>
        )}
      </Index>
    </section>
  );
}

/**
 * The PTY sessions working on this feature, which is not the same list as the
 * steps above it.
 *
 * A step run as a job has no terminal by design, so an empty list under a
 * running step means "this one is headless", not "nothing started".
 */
function AgentsCard(props: { rows: ReturnType<typeof harnessFeatureAgents>; headless: boolean }) {
  const pinned = () => harnessStore.previewSession;

  function preview(session: string): void {
    if (pinned() === session) {
      setHarnessStore({ previewSession: null, previewFull: false });
      void harness.detachPreview().catch(() => undefined);
      return;
    }
    setHarnessStore("previewSession", session);
    void harness.attachPreview(session).catch(() => undefined);
  }

  return (
    <section class="feature-agents">
      <h3>Terminal sessions</h3>
      <For
        each={props.rows}
        fallback={
          <p class="panel-note">
            {props.headless
              ? "This feature runs headless — a step has no terminal. Show a step's details above to watch what it is doing."
              : "No agent sessions yet. The orchestrator will appear here once launched."}
          </p>
        }
      >
        {(row) => (
          <div
            class="feature-agent-row"
            classList={{ pinned: pinned() === row.session.id }}
            style={{ "--depth": String(row.depth) }}
          >
            <StateMarker session={row.session} />
            <SessionGlyph providerId={row.session.agent_provider_id} session={row.session} />
            <button
              type="button"
              class="forge-row feature-agent"
              onClick={() => void selectSession(row.session.id).catch(() => undefined)}
            >
              {row.label}
            </button>
            {/* Watching is not the same as switching to it: the preview keeps
                the terminal pane where the user was working. */}
            <Button variant="secondary" size="xs" onClick={() => preview(row.session.id)}>
              {pinned() === row.session.id ? "Hide preview" : "Show preview"}
            </Button>
            <Button
              variant="secondary"
              size="xs"
              onClick={() => void selectSession(row.session.id).catch(() => undefined)}
            >
              Tab
            </Button>
            <Show when={pinned() === row.session.id}>
              <Button
                variant="secondary"
                size="xs"
                onClick={() => setHarnessStore("previewFull", !harnessStore.previewFull)}
              >
                {harnessStore.previewFull ? "Collapse" : "Expand"}
              </Button>
            </Show>
          </div>
        )}
      </For>
    </section>
  );
}

function ActionsRow(props: { feature: HarnessFeature; running: boolean }) {
  const project = () => harnessStore.project;
  const [starting, setStarting] = createSignal(false);
  const step = createMemo(() => runnableStep(props.feature, props.running));
  const busy = () => props.running || starting();

  // Clear the click-local busy flag once a live job lands (or the feature
  // leaves a runnable status). Leaving it latched would keep a spinner after
  // a refused start with no job to replace it.
  createEffect(() => {
    if (props.running || step() === null) setStarting(false);
  });

  return (
    <div class="feature-actions">
      <Show when={step()}>
        {(next) => (
          <Button
            variant="primary"
            loading={starting()}
            onClick={() => {
              const id = project();
              if (!id) return;
              setStarting(true);
              void harness
                .runStep(id, props.feature.id, next().step)
                .catch(() => setStarting(false));
            }}
          >
            {next().label}
          </Button>
        )}
      </Show>
      <Show when={props.feature.orchestrator_session_id}>
        {(session) => (
          <Button
            variant="secondary"
            onClick={() => void selectSession(session()).catch(() => undefined)}
          >
            Open orchestrator
          </Button>
        )}
      </Show>
      {/* The patch the feature is producing, in the tab that reads patches —
          not inside this one, the same rule PR compose follows. */}
      <Button variant="secondary" onClick={() => openDiff()}>
        Show diff
      </Button>
      <Button
        variant="secondary"
        onClick={() => {
          const id = project();
          if (id) void harness.validateHarness(id).catch(() => undefined);
        }}
      >
        Validate harness
      </Button>
      <Show when={busy()}>
        <Progress
          label={props.running ? "A step is running" : "Starting step…"}
          class="feature-run-progress"
        />
      </Show>
    </div>
  );
}

function Timeline() {
  return (
    <section class="feature-timeline">
      <h3>Timeline</h3>
      <For each={harnessStore.timeline} fallback={<p class="panel-note">No events yet.</p>}>
        {(event) => (
          <div class="feature-timeline-row">
            <time class="feature-timeline-ts" dateTime={event.ts} title={event.ts}>
              {new Date(event.ts).toLocaleString()}
            </time>
            <span class="feature-timeline-kind">{event.kind.replaceAll("_", " ")}</span>
            <Show when={event.detail}>
              {(detail) => (
                <details class="feature-timeline-detail">
                  <summary>{eventSummary(detail())}</summary>
                  <pre>{detail()}</pre>
                </details>
              )}
            </Show>
            {/* The one field a reader acts on rather than prints: jobs are
                forgotten across a daemon restart, and this still points at one. */}
            <Show when={event.job}>
              {(job) => (
                <Button
                  variant="secondary"
                  size="xs"
                  onClick={() => setHarnessStore("openJob", job())}
                >
                  Show details
                </Button>
              )}
            </Show>
          </div>
        )}
      </For>
    </section>
  );
}
