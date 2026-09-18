import { For, Show, createMemo } from "solid-js";
import * as harness from "../../features/harness/api";
import { gatedFeatures, type HarnessFeature } from "../../contracts/harness";
import { harnessStore, openFeature } from "../../features/harness/harnessStore";
import { openFeatureView } from "../../navigation/viewsStore";
import { Button } from "../../ui/index";

export function AttentionBar() {
  const gated = createMemo(() => gatedFeatures(harnessStore.features));
  const lead = () => gated()[0] ?? null;
  const waiting = () => Math.max(0, gated().length - 1);

  function advance(id: number, action: Parameters<typeof harness.advance>[2]): void {
    const project = harnessStore.project;
    if (!project) return;
    // The revision the strip is looking at, so a click on a row the daemon has
    // already moved past is answered rather than obeyed.
    const revision = harnessStore.features.find((f) => f.id === id)?.revision ?? null;
    void harness.advance(project, id, action, revision).catch(() => undefined);
  }

  function open(feature: HarnessFeature): void {
    openFeature(feature.id);
    openFeatureView(feature.id);
    const project = harnessStore.project;
    if (project) void harness.loadFeatureDetail(project, feature.id).catch(() => undefined);
  }

  function documents(feature: HarnessFeature): void {
    open(feature);
    const project = harnessStore.project;
    if (project) void harness.loadArtifact(project, feature.id, "Gate").catch(() => undefined);
  }

  return (
    <Show when={lead()}>
      {(feature) => (
        <div class="attention-bar" role="status">
          <span class="attention-label">ATTENTION</span>
          <span class="attention-badge">Approval</span>
          <button type="button" class="forge-row attention-feature" onClick={() => open(feature())}>
            #{feature().id} · {feature().slug}
          </button>
          {/* One feature at a time with a count of the rest: a strip that grew
              a row per gate would push the work off the screen to report that
              there is work. */}
          <Show when={waiting() > 0}>
            <span class="attention-more">+{waiting()} more waiting</span>
          </Show>
          {/* The three documents the decision rests on, without asking the
              reader to open the tab and then find the reader inside it. */}
          <Button
            variant="ghost"
            size="sm"
            class="attention-action"
            onClick={() => documents(feature())}
          >
            Documents
          </Button>
          <Button
            variant="ghost"
            size="sm"
            class="attention-action"
            disabled={harnessStore.advancing}
            onClick={() => advance(feature().id, "ApproveSpec")}
          >
            Approve
          </Button>
          <Button
            variant="ghost"
            size="sm"
            class="attention-action"
            disabled={harnessStore.advancing}
            onClick={() => advance(feature().id, "ReviseSpec")}
          >
            Reject
          </Button>
        </div>
      )}
    </Show>
  );
}
