import { createMemo, createSignal } from "solid-js";
import { forgeStore } from "../../state/forgeStore";
import { pickFile } from "../../runtime/host";
import type { RunSpecRequest } from "./dialogs";
import { launchSpec } from "./sessionActions";
import { SessionGlyph } from "./SessionGlyph";
import {
  Button,
  Dialog,
  Disclosure,
  Select,
  TextArea,
  TextField,
  toast,
  type SelectOption,
} from "../../ui/index";

type Preset = "claude" | "mix";

const PRESETS: SelectOption<Preset>[] = [
  { value: "claude", label: "Claude coder, medium effort" },
  { value: "mix", label: "Cursor coder" },
];

const ROUNDS: SelectOption<string>[] = ["1", "2", "3", "4", "5"].map((value) => ({
  value,
  label: value === "1" ? "1 round" : `${value} rounds`,
}));

const PRESET_CODER = "";

export function RunSpecDialog(props: { request: RunSpecRequest; onDismiss: () => void }) {
  const [spec, setSpec] = createSignal(props.request.spec);
  const [preset, setPreset] = createSignal<Preset>("claude");
  const [coder, setCoder] = createSignal(PRESET_CODER);
  const [worktree, setWorktree] = createSignal("");
  const [rounds, setRounds] = createSignal("3");
  const [starting, setStarting] = createSignal(false);

  const checkout = createMemo(() =>
    forgeStore.workspaces.find((item) => item.id === props.request.workspace),
  );

  // One row per provider: the planner launches the coder by provider, never by profile.
  const coders = createMemo<SelectOption<string>[]>(() => {
    const seen = new Set<string>();
    const rows: SelectOption<string>[] = [{ value: PRESET_CODER, label: "The preset's coder" }];
    for (const item of forgeStore.launchables) {
      if (item.kind !== "agent" || item.profile !== null || !item.provider) continue;
      if (seen.has(item.provider)) continue;
      seen.add(item.provider);
      rows.push({
        value: item.provider,
        label: item.label,
        disabled: !item.enabled || !item.supports_initial_prompt,
        glyph: <SessionGlyph providerId={item.provider} size={14} />,
      });
    }
    return rows;
  });

  const blocked = () => starting() || spec().trim() === "";

  async function choose(): Promise<void> {
    const path = await pickFile("Choose a spec", checkout()?.path);
    if (path) setSpec(path);
  }

  function start(): void {
    if (blocked()) return;
    setStarting(true);
    void launchSpec({
      workspace: props.request.workspace,
      spec: spec().trim(),
      preset: preset(),
      coder: coder() || null,
      worktree: worktree().trim() || null,
      rounds: Number(rounds()),
    })
      .then(() => props.onDismiss())
      .catch((error: unknown) => {
        setStarting(false);
        toast({
          title: "Could not start the spec run.",
          detail: error instanceof Error ? error.message : undefined,
          tone: "danger",
        });
      });
  }

  return (
    <Dialog
      title="Run a spec"
      size="md"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" disabled={starting()} onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="primary" disabled={blocked()} onClick={start}>
            {starting() ? "Starting…" : "Start planner"}
          </Button>
        </>
      }
    >
      <p class="panel-note">
        A Claude planner validates the spec, asks what it cannot answer, plans, starts a coder under
        itself and reviews its work.
      </p>
      <TextArea
        label="Spec"
        value={spec()}
        onChange={setSpec}
        placeholder="docs/spec.md, a Claude artifact link, or the spec itself"
        rows={3}
        autoResize
      />
      <Button size="sm" onClick={() => void choose()}>
        Choose a file…
      </Button>
      <Select label="Preset" value={preset()} options={PRESETS} onChange={setPreset} />
      <Disclosure summary={<span class="tree-label forge-truncate">Coder and review</span>}>
        <Select label="Coder" value={coder()} options={coders()} onChange={setCoder} />
        <TextField
          label="Worktree branch"
          value={worktree()}
          onChange={setWorktree}
          placeholder="Empty: the coder works in this checkout"
        />
        <Select label="Review rounds" value={rounds()} options={ROUNDS} onChange={setRounds} />
      </Disclosure>
      <p class="panel-note">
        Starts in <span class="session-changes-base">{checkout()?.path ?? ""}</span>
      </p>
    </Dialog>
  );
}
