import { Progress as Kobalte } from "@kobalte/core/progress";
import { Show } from "solid-js";

export type ProgressProps = {
  /** Announced name — "Reading the checkout", not "Progress". */
  label: string;
  /** Omit for an indeterminate bar: work is happening, of unknown length. */
  value?: number;
  max?: number;
  /** Shown at the end of the label line. */
  detail?: string;
  /** `warning` for a budget running low; the detail text still says how low. */
  tone?: "accent" | "warning";
  class?: string;
};

export function Progress(props: ProgressProps) {
  return (
    <Kobalte
      class={`forge-progress ${props.class ?? ""}`}
      value={props.value}
      minValue={0}
      maxValue={props.max ?? 100}
      indeterminate={props.value === undefined}
      data-tone={props.tone ?? "accent"}
    >
      <div class="forge-progress-head">
        <Kobalte.Label class="forge-progress-label">{props.label}</Kobalte.Label>
        <Show
          when={props.detail}
          fallback={<Kobalte.ValueLabel class="forge-hint forge-progress-value" />}
        >
          {(text) => <span class="forge-hint forge-progress-value">{text()}</span>}
        </Show>
      </div>
      <Kobalte.Track class="forge-progress-track">
        <Kobalte.Fill class="forge-progress-fill" />
      </Kobalte.Track>
    </Kobalte>
  );
}
