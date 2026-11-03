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

const ROUNDS: SelectOption<string>[] = ["1", "2", "3", "4", "5"].map((value) => ({
  value,
  label: value === "1" ? "1 round" : `${value} rounds`,
}));

// The claude preset pins medium effort; any other provider is the mix preset with that coder.
const CLAUDE_PRESET = "claude";

export function RunSpecDialog(props: { request: RunSpecRequest; onDismiss: () => void }) {
  const [spec, setSpec] = createSignal(props.request.spec);
  const [coder, setCoder] = createSignal(CLAUDE_PRESET);
  const [worktree, setWorktree] = createSignal("");
  const [rounds, setRounds] = createSignal("3");
  const [starting, setStarting] = createSignal(false);

  const checkout = createMemo(() =>
    forgeStore.workspaces.find((item) => item.id === props.request.workspace),
  );

  // One row per provider: the planner launches the coder by provider, never by profile.
  const coders = createMemo<SelectOption<string>[]>(() => {
    const seen = new Set<string>();
    const rows: SelectOption<string>[] = [];
    for (const item of forgeStore.launchables) {
      if (item.kind !== "agent" || item.profile !== null || !item.provider) continue;
      if (seen.has(item.provider)) continue;
      seen.add(item.provider);
      const claude = item.provider === CLAUDE_PRESET;
      const row: SelectOption<string> = {
        value: item.provider,
        label: claude ? `${item.label}, medium effort` : item.label,
        disabled: !item.enabled || !item.supports_initial_prompt,
        glyph: <SessionGlyph providerId={item.provider} size={14} />,
      };
      if (claude) rows.unshift(row);
      else rows.push(row);
    }
    if (!seen.has(CLAUDE_PRESET)) {
      rows.unshift({ value: CLAUDE_PRESET, label: "Claude, medium effort" });
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
      preset: coder() === CLAUDE_PRESET ? "claude" : "mix",
      coder: coder() === CLAUDE_PRESET ? null : coder(),
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
      <Select label="Coder" value={coder()} options={coders()} onChange={setCoder} />
      <Disclosure summary={<span class="tree-label forge-truncate">Worktree and review</span>}>
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
