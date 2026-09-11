import { For, Show, createMemo } from "solid-js";
import type { Job } from "../runtime/types";
import { Icon } from "../theme/icons";
import { Progress } from "../ui";
import type { HarnessFeature } from "./types";
import { featureProgress } from "./progress";

export function FeatureProgress(props: {
  feature: HarnessFeature;
  jobs: Job[];
  compact?: boolean;
}) {
  const progress = createMemo(() => featureProgress(props.feature, props.jobs));
  const active = createMemo(() =>
    progress().stages.some((stage) => stage.state === "running" || stage.state === "queued"),
  );
  return (
    <section
      class="harness-progress"
      classList={{ compact: props.compact, active: active() }}
      aria-label="Feature progress"
      aria-busy={active() || undefined}
    >
      <p class="harness-progress-summary">{progress().summary}</p>
      <Show when={active() && !props.compact}>
        <Progress label="Step in progress" class="harness-progress-bar" />
      </Show>
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
                <Show
                  when={stage.state === "running" || stage.state === "queued"}
                  fallback={stage.state === "complete" ? "✓" : index() + 1}
                >
                  <Icon
                    name="loader"
                    class="forge-icon-spin forge-icon-blue"
                    size={12}
                    title={stage.state === "queued" ? "Queued" : "Running"}
                  />
                </Show>
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
