import { For, Show, createMemo } from "solid-js";
import * as harness from "../harness/api";
import { gatedFeatures, type HarnessFeature } from "../harness/types";
import { harnessStore, openFeature } from "../store/harnessStore";
import { openFeatureView } from "../store/viewsStore";
import { Button } from "../ui";

/**
 * The strip that says a feature is waiting on a person (§16.3).
 *
 * The rail answers "which *session* wants me"; this answers the other half —
 * which *feature* stopped at the human gate. The two are the same idea at
 * different scales, and the gate is the one that could be missed for hours: an
 * agent asking a question rings a bell and lights the rail, while a spec that
 * reached the gate only changes a word in a badge inside a panel nobody had
 * open.
 *
 * It sits above whatever the centre column is showing, deliberately: the
 * decision belongs to the operator wherever they happen to be looking, and
 * making them find the feature tab first is the friction this removes.
 * Approving from here does exactly what approving inside the tab does — the
 * daemon runs the next step — so there is one flow with two doors.
 */
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
