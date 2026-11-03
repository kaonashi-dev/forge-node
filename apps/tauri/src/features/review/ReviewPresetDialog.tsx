import { For, Show, createEffect, createMemo, createSignal, onMount, untrack } from "solid-js";
import type { Tier } from "../../contracts/workbench";
import { forgeStore } from "../../state/forgeStore";
import { writeChoice } from "../../state/preferences";
import { Button, Dialog, Select, TextArea, toast, type SelectOption } from "../../ui/index";
import { loadReviewConfig, startReview } from "./commands";
import { initialPreset, presetKey } from "./reviewPreset";
import { reviewStore } from "./state";

const PRESET_TIER = "";

const TIERS: SelectOption<Tier | typeof PRESET_TIER>[] = [
  { value: PRESET_TIER, label: "Preset's tier" },
  { value: "base", label: "Base" },
  { value: "plus", label: "Plus" },
  { value: "apex", label: "Apex" },
];

export function ReviewPresetDialog(props: { workspace: string; onDismiss: () => void }) {
  const [preset, setPreset] = createSignal("");
  const [tier, setTier] = createSignal<Tier | typeof PRESET_TIER>(PRESET_TIER);
  const [instructions, setInstructions] = createSignal("");

  const projectId = () => forgeStore.workspaces.find((w) => w.id === props.workspace)?.project_id;
  const config = () => reviewStore.configs[props.workspace];
  const presets = createMemo<SelectOption<string>[]>(() =>
    (config()?.presets ?? []).map((item) => ({ value: item.id, label: item.name })),
  );

  onMount(() => void loadReviewConfig(props.workspace).catch(() => undefined));
  // Only a (re)loaded config preselects: another preference written while the
  // dialog is open must not undo the person's pick.
  createEffect(() => {
    const settings = config();
    if (!settings) return;
    untrack(() => {
      const project = projectId();
      const remembered = project ? forgeStore.app_state[presetKey(project)] : undefined;
      setPreset(initialPreset(settings, remembered));
    });
  });

  function start(): void {
    if (!config()) return;
    const project = projectId();
    if (project) writeChoice(presetKey(project), preset());
    void startReview(props.workspace, {
      preset: preset(),
      tier: tier() || null,
      instructions: instructions().trim() || null,
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
          <Button variant="primary" disabled={!config()} onClick={start}>
            Review
          </Button>
        </>
      }
    >
      <Select label="Preset" value={preset()} options={presets()} onChange={setPreset} />
      <Select label="Tier" value={tier()} options={TIERS} onChange={setTier} />
      <TextArea
        label="Instructions"
        value={instructions()}
        onChange={setInstructions}
        placeholder="Optional: what this review should focus on"
        rows={3}
        autoResize
      />
      <Show when={config()?.errors.length}>
        <ul class="panel-note">
          <For each={config()?.errors}>{(error) => <li>{error}</li>}</For>
        </ul>
      </Show>
    </Dialog>
  );
}
