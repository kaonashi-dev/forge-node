import { For, Show, createEffect, createMemo, createSignal, onMount, untrack } from "solid-js";
import { forgeStore } from "../../state/forgeStore";
import { writeChoice } from "../../state/preferences";
import { Button, Dialog, RadioGroup, Select, TextArea, TextField, toast } from "../../ui/index";
import { loadReviewConfig, startReview } from "./commands";
import {
  CUSTOM_PRESET,
  REVIEW_CUSTOM_KEY,
  REVIEW_PROVIDER_KEY,
  initialPreset,
  launchOption,
  preferredReviewer,
  presetKey,
  readyToReview,
  reviewHint,
  reviewers,
} from "./reviewPreset";
import { reviewStore } from "./state";

export function ReviewPresetDialog(props: { workspace: string; onDismiss: () => void }) {
  const [presetId, setPresetId] = createSignal("");
  const [reviewerId, setReviewerId] = createSignal<string | null>(null);
  const [custom, setCustom] = createSignal("");
  const [model, setModel] = createSignal("");
  const [effort, setEffort] = createSignal("");

  const projectId = () => forgeStore.workspaces.find((w) => w.id === props.workspace)?.project_id;
  const config = () => reviewStore.configs[props.workspace];
  const preset = createMemo(() => config()?.presets.find((item) => item.id === presetId()) ?? null);
  const candidates = createMemo(() => reviewers(forgeStore.providers));
  const reviewer = createMemo(() => preferredReviewer(candidates(), reviewerId()));
  const isCustom = () => preset()?.id === CUSTOM_PRESET;

  onMount(() => {
    void loadReviewConfig(props.workspace).catch(() => undefined);
    // Read once: a preference that re-applied itself would fight the picker.
    setReviewerId(forgeStore.app_state[REVIEW_PROVIDER_KEY] ?? null);
    setCustom(forgeStore.app_state[REVIEW_CUSTOM_KEY] ?? "");
  });
  // Only a (re)loaded config preselects: another preference written while the
  // dialog is open must not undo the person's pick.
  createEffect(() => {
    const settings = config();
    if (!settings) return;
    untrack(() => {
      const project = projectId();
      const remembered = project ? forgeStore.app_state[presetKey(project)] : undefined;
      setPresetId(initialPreset(settings, remembered));
    });
  });

  function start(): void {
    const picked = reviewer();
    if (!picked || !readyToReview(picked, preset(), custom())) return;
    const project = projectId();
    if (project) writeChoice(presetKey(project), presetId());
    writeChoice(REVIEW_PROVIDER_KEY, picked.id);
    if (isCustom()) writeChoice(REVIEW_CUSTOM_KEY, custom());
    void startReview(props.workspace, {
      preset: presetId(),
      provider: picked.id,
      instructions: isCustom() ? custom().trim() : null,
      model: launchOption(model()),
      effort: launchOption(effort()),
    })
      .then(() => props.onDismiss())
      .catch((error: unknown) =>
        toast({
          title: "Could not start the review.",
          detail: error instanceof Error ? error.message : undefined,
          tone: "danger",
        }),
      );
  }

  return (
    <Dialog
      title="Review with a preset"
      size="md"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={!readyToReview(reviewer(), preset(), custom())}
            onClick={start}
          >
            Start review
          </Button>
        </>
      }
    >
      <section class="pr-compose-tasks">
        <RadioGroup
          label="What to ask for"
          itemClass="forge-card pr-task-row"
          value={presetId()}
          onChange={setPresetId}
          options={(config()?.presets ?? []).map((option) => ({
            value: option.id,
            label: option.name,
            render: () => (
              <>
                <span class="pr-task-name">{option.name}</span>
                <Show when={option.detail}>
                  <span class="pr-task-detail">{option.detail}</span>
                </Show>
              </>
            ),
          }))}
        />
      </section>
      <Show when={isCustom()}>
        <TextArea
          class="pr-compose-field"
          label="Ask for"
          rows={4}
          value={custom()}
          onChange={setCustom}
          placeholder="What should the agent look for?"
        />
      </Show>
      <Select
        label="Agent"
        aria-label="Agent to review with"
        value={reviewer()?.id ?? null}
        placeholder="No agent can review"
        options={candidates().map((option) => ({
          value: option.id,
          label: `${option.name} · ${option.mode}`,
        }))}
        onChange={setReviewerId}
      />
      <TextField
        class="pr-compose-field"
        label="Model"
        value={model()}
        onChange={setModel}
        placeholder="Provider default"
      />
      <TextField
        class="pr-compose-field"
        label="Effort"
        value={effort()}
        onChange={setEffort}
        placeholder="Provider default"
      />
      <p class="settings-hint pr-compose-hint">{reviewHint(reviewer(), preset(), custom())}</p>
      <Show when={config()?.errors.length}>
        <ul class="panel-note">
          <For each={config()?.errors}>{(error) => <li>{error}</li>}</For>
        </ul>
      </Show>
    </Dialog>
  );
}
