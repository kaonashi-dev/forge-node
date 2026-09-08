import { For, createMemo } from "solid-js";
import type { Job } from "../runtime/types";
import type { HarnessFeature } from "./types";
import { featureProgress } from "./progress";

export function FeatureProgress(props: {
  feature: HarnessFeature;
  jobs: Job[];
  compact?: boolean;
}) {
  const progress = createMemo(() => featureProgress(props.feature, props.jobs));
  return (
    <section
      class="harness-progress"
      classList={{ compact: props.compact }}
      aria-label="Feature progress"
    >
      <p class="harness-progress-summary">{progress().summary}</p>
      <ol class="harness-stages">
        <For each={progress().stages}>
          {(stage, index) => (
            <li
              class="harness-stage"
              data-state={stage.state}
              aria-label={`${stage.label}: ${stage.state}`}
              title={`${stage.label}: ${stage.state}`}
              aria-current={
                ["running", "queued", "waiting", "ready", "blocked"].includes(stage.state)
                  ? "step"
                  : undefined
              }
            >
              <span class="harness-stage-number">
                {stage.state === "complete" ? "✓" : index() + 1}
              </span>
              <span class="harness-stage-label">
                {stage.label}
                <small>{stage.state}</small>
              </span>
            </li>
          )}
        </For>
      </ol>
    </section>
  );
}
