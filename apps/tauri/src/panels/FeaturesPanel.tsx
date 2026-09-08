import { FeatureProgress } from "../harness/FeatureProgress";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import * as harness from "../harness/api";
import {
  FEATURE_FILTERS,
  featureLabel,
  featurePhase,
  statusBadge,
  type FeatureFilter,
  type HarnessFeature,
} from "../harness/types";
import { jobStateTone, jobStepLabel } from "../harness/steps";
import { jobPreviewLine } from "../harness/stream";
import {
  agentCount,
  clip,
  counts,
  detailRows,
  emptyReason,
  featureSteps,
  metaFacts,
  visibleFeatures,
  type FeatureScope,
} from "./featureRows";
import { forgeStore } from "../store/forgeStore";
import {
  focusHarnessProject,
  harnessStore,
  openFeature,
  setHarnessStore,
} from "../store/harnessStore";
import { workbenchStore } from "../store/workbenchStore";
import { openFeatureCompose, openFeatureView } from "../store/viewsStore";
import { harnessFeatureAgents } from "../shell/sessionTree";
import { selectSession } from "../runtime/api";
import { Icon, SessionGlyph, StateMarker } from "../theme/icons";
import { jobStateLabel } from "../workbench/JobStreamView";
import type { Job } from "../runtime/types";
import { Badge, Button, EmptyState, FilterHeader, IconButton, ListCard, Tooltip } from "../ui";

/**
 * Right panel: harness features, and the agents working on each one.
 *
 * Sibling of the History panel and built from the same parts in the same
 * order — scope, status, free search, then cards. What differs is what a card
 * *is*: a history card is one run, read and left; a feature card is a piece of
 * work several agents are on at once, so it opens onto its agents.
 */

/** Characters of a step's last line a card shows. */
const LINE_CLIP = 96;

/**
 * How far the panel reaches around the checkout on screen.
 *
 * Two levels where History has three, because `harness/features.json` is one
 * file per *repository*: `Project` already is everything the panel has read,
 * and an `All` segment would promise a sweep across projects that nothing on
 * this side has performed.
 *
 * `Project` is the default and not the narrower one: a feature registered from
 * the CLI carries no `workspace_id` at all, so `Workspace` would open on an
 * empty list for every repository whose features were registered outside Forge.
 */
const SCOPES: FeatureScope[] = ["Workspace", "Project"];

export function FeaturesPanel() {
  const [scope, setScope] = createSignal<FeatureScope>("Project");
  const [filter, setFilter] = createSignal<FeatureFilter>("All");
  const [query, setQuery] = createSignal("");
  const [expanded, setExpanded] = createSignal<ReadonlySet<number>>(new Set());

  const workspace = () =>
    forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace) ?? null;
  const project = () => workspace()?.project_id ?? null;

  createEffect(() => {
    focusHarnessProject(project());
  });

  // One read per project, when the panel first has one. The list is a file the
  // daemon parses, so re-reading it on every redraw would be a parse per frame.
  createEffect(() => {
    const id = project();
    if (!id || harnessStore.loadingList || harnessStore.features.length > 0) return;
    if (harnessStore.listError) return;
    void harness.loadFeatures(id).catch(() => undefined);
  });

  const features = createMemo(() =>
    visibleFeatures({
      features: harnessStore.features,
      scope: scope(),
      anchor: workspace()?.id ?? null,
      filter: filter(),
      search: query().trim().toLowerCase(),
    }),
  );

  function open(feature: HarnessFeature): void {
    openFeature(feature.id);
    openFeatureView(feature.id);
    const id = project();
    if (id) void harness.loadFeatureDetail(id, feature.id).catch(() => undefined);
  }

  /** Open a feature *on* one of its steps: the stream is the answer wanted. */
  function openStep(feature: HarnessFeature, job: string): void {
    open(feature);
    setHarnessStore("openJob", job);
  }

  function toggle(id: number): void {
    setExpanded((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }

  return (
    <div class="panel-body">
      <FilterHeader
        label="Filter features"
        placeholder="Filter features…"
        query={query()}
        onQuery={setQuery}
        rows={[
          {
            label: "Scope",
            value: scope(),
            onChange: (value) => setScope(value as FeatureScope),
            options: SCOPES.map((value) => ({ value })),
          },
          {
            label: "Status filter",
            value: filter(),
            onChange: (value) => setFilter(value as FeatureFilter),
            options: FEATURE_FILTERS.map((value) => ({ value })),
          },
        ]}
      >
        <Tooltip label="Draft a new feature" contents>
          <Button variant="secondary" size="xs" onClick={openFeatureCompose}>
            <Icon name="plus" class="forge-icon-muted" size={13} />
            New
          </Button>
        </Tooltip>
        <Tooltip label="Re-read features.json" contents>
          <IconButton
            label="Re-read features.json"
            disabled={harnessStore.loadingList}
            onClick={() => {
              const id = project();
              if (id) void harness.loadFeatures(id).catch(() => undefined);
            }}
          >
            <Icon name="refresh" class="forge-icon-muted" size={13} />
          </IconButton>
        </Tooltip>
      </FilterHeader>

      {/* The two numbers the history panel also reports: how much of the file
          is on screen, and whether the read has finished. */}
      <p class="panel-note features-counts">
        {counts(features().length, harnessStore.features.length, harnessStore.loadingList)}
      </p>

      <Show when={harnessStore.listError}>{(error) => <p class="panel-error">{error()}</p>}</Show>

      <For
        each={features()}
        fallback={
          <EmptyState
            message={emptyMessage(
              project(),
              harnessStore.initialized,
              harnessStore.features.length,
              query().trim(),
            )}
          />
        }
      >
        {(feature) => (
          <FeatureCard
            feature={feature}
            expanded={expanded().has(feature.id)}
            onToggle={() => toggle(feature.id)}
            onOpen={open}
            onOpenStep={openStep}
          />
        )}
      </For>
    </div>
  );
}

function emptyMessage(
  project: string | null,
  initialized: boolean,
  total: number,
  search: string,
): string {
  const reason = emptyReason(project, initialized, total, search.toLowerCase());
  return `${reason.title}. ${reason.hint}`;
}

function FeatureCard(props: {
  feature: HarnessFeature;
  expanded: boolean;
  onToggle: () => void;
  onOpen: (feature: HarnessFeature) => void;
  onOpenStep: (feature: HarnessFeature, job: string) => void;
}) {
  const agents = createMemo(() =>
    harnessFeatureAgents(forgeStore.sessions, props.feature.orchestrator_session_id),
  );
  const steps = createMemo(() => featureSteps(harnessStore.jobs, props.feature.id));
  // Only the number is per redraw: the rows themselves are built for an open
  // card, and resolving a title per agent to reach a `length` is an allocation
  // per agent per frame.
  const roster = createMemo(() =>
    agentCount(forgeStore.sessions, harnessStore.jobs, props.feature),
  );
  const facts = createMemo(() =>
    metaFacts(
      props.feature,
      forgeStore.sessions,
      harnessStore.jobs,
      forgeStore.workspaces,
      new Date(),
    ),
  );

  return (
    <ListCard
      class={`feature-card phase-${featurePhase(props.feature)}`}
      openLabel={`Open feature #${props.feature.id}`}
      onOpen={() => props.onOpen(props.feature)}
      glyph={<span class="feature-id">#{props.feature.id}</span>}
      title={featureLabel(props.feature)}
      aside={
        <>
          <span class="feature-status">{statusBadge(props.feature.status)}</span>
          {/* Its own control, and it stops the click: the card underneath
              means "open this feature", which is not what a chevron says. */}
          <button
            type="button"
            class="forge-row feature-expand"
            aria-expanded={props.expanded}
            aria-label={props.expanded ? "Hide agents" : "Show agents"}
            onClick={(event) => {
              event.stopPropagation();
              props.onToggle();
            }}
          >
            <Icon
              name={props.expanded ? "chevron-down" : "chevron-right"}
              class="forge-icon-faint"
              size={13}
            />
          </button>
        </>
      }
      meta={
        <For each={facts()}>
          {(fact, index) => (
            <span class="feature-fact">
              <Show when={index() > 0}>
                <span class="feature-fact-sep">·</span>
              </Show>
              {fact}
            </span>
          )}
        </For>
      }
    >
      <FeatureProgress feature={props.feature} jobs={steps()} compact />
      <Show when={!props.expanded && roster() > 0}>
        <p class="panel-note">{roster()} on this feature — open the card to see them.</p>
      </Show>

      <Show when={props.expanded}>
        <section class="feature-roster">
          <h4>Agents</h4>
          <Show
            when={agents().length > 0 || steps().length > 0}
            fallback={<p class="panel-note">No agent is on record with this daemon.</p>}
          >
            {/* Two kinds, because the harness has two: a session is an agent
                with a PTY somebody can watch; a step is a headless job with an
                exit code, so its row opens the tab on that step's stream. */}
            <For each={agents()}>
              {(row) => (
                <Tooltip label="Open this session" contents>
                  <button
                    type="button"
                    class="forge-row feature-agent"
                    style={{ "--depth": String(row.depth) }}
                    onClick={(event) => {
                      event.stopPropagation();
                      void selectSession(row.session.id).catch(() => undefined);
                    }}
                  >
                    <StateMarker session={row.session} />
                    <SessionGlyph
                      providerId={row.session.agent_provider_id}
                      session={row.session}
                    />
                    <span class="tree-label">{row.label}</span>
                  </button>
                </Tooltip>
              )}
            </For>
            <For each={steps()}>
              {(job) => (
                <StepRow job={job} onOpen={() => props.onOpenStep(props.feature, job.id)} />
              )}
            </For>
          </Show>
        </section>

        <dl class="feature-details">
          <For each={detailRows(props.feature, forgeStore.workspaces)}>
            {(row) => (
              <>
                <dt>{row.label}</dt>
                <dd>{row.value}</dd>
              </>
            )}
          </For>
        </dl>
      </Show>
    </ListCard>
  );
}

/** One headless step, with the last thing it said. */
function StepRow(props: { job: Job; onOpen: () => void }) {
  const note = () => jobPreviewLine(props.job.last_line, props.job.summary);

  return (
    <Tooltip label="Show this step's details" contents>
      <button
        type="button"
        class="forge-row feature-step"
        // Under the orchestrator that asked for it, the same indent the
        // session tree gives a child.
        style={{ "--depth": String(props.job.parent_session_id === null ? 0 : 1) }}
        onClick={(event) => {
          event.stopPropagation();
          props.onOpen();
        }}
      >
        <Icon name="agent" class={`job-glyph state-${props.job.state.toLowerCase()}`} size={13} />
        <span class="feature-step-body">
          <span class="tree-label">
            {jobStepLabel(props.job.role)} · {props.job.provider_id}
          </span>
          <Show when={note()}>
            {(line) => <span class="feature-job-line">{clip(line(), LINE_CLIP)}</span>}
          </Show>
        </span>
        <Badge tone={jobStateTone(props.job.state)}>{jobStateLabel(props.job)}</Badge>
      </button>
    </Tooltip>
  );
}
