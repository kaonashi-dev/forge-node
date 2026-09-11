import { Show, createMemo, createSignal } from "solid-js";
import { createChildSession } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";
import type { SpawnChildRequest } from "../store/runtimeStore";
import { SessionGlyph } from "../theme/icons";
import { Button, Dialog, Select, TextArea, type SelectOption, toast } from "../ui";

type Target = { provider: string; profile: string | null };

const ROLE_OPTIONS: SelectOption<string>[] = [
  { value: "generic", label: "Generic" },
  { value: "planner", label: "Planner" },
  { value: "researcher", label: "Researcher" },
  { value: "executor", label: "Executor" },
  { value: "reviewer", label: "Reviewer" },
  { value: "tester", label: "Tester" },
];

const WORKSPACE_OPTIONS: SelectOption<string>[] = [
  { value: "same", label: "Same checkout" },
  { value: "worktree", label: "New managed worktree" },
];

/**
 * Spawn a child agent under the current session with any installed provider.
 *
 * Unlike handoff, the prompt is typed here (not captured from the terminal),
 * and the graph edge is always recorded via `CreateChildSession`.
 */
export function SpawnChildDialog(props: { request: SpawnChildRequest; onDismiss: () => void }) {
  const [target, setTarget] = createSignal<string | null>(null);
  const [prompt, setPrompt] = createSignal("");
  const [role, setRole] = createSignal("generic");
  const [workspace, setWorkspace] = createSignal("same");
  const [starting, setStarting] = createSignal(false);

  const options = createMemo<SelectOption<string>[]>(() =>
    forgeStore.launchables
      .filter((item) => item.kind === "agent" && item.enabled && item.provider !== null)
      .map((item) => ({
        value: item.key,
        label: item.label,
        glyph: <SessionGlyph providerId={item.provider} size={14} />,
      })),
  );

  const chosen = () => target() ?? options()[0]?.value ?? null;

  function resolveTarget(key: string): Target | null {
    const row = forgeStore.launchables.find((item) => item.key === key);
    if (!row?.provider) return null;
    return { provider: row.provider, profile: row.profile };
  }

  const blocked = () => starting() || chosen() === null || options().length === 0;

  function start(): void {
    const key = chosen();
    if (!key) return;
    const to = resolveTarget(key);
    if (!to) return;
    setStarting(true);
    void createChildSession({
      parent: props.request.parent,
      provider: to.provider,
      profile: to.profile,
      prompt: prompt().trim() || null,
      role: role(),
      workspacePolicy: workspace() === "worktree" ? "worktree" : "same",
    })
      .then(() => props.onDismiss())
      .catch((error: unknown) => {
        setStarting(false);
        toast({
          title: "Could not spawn the child session.",
          detail: error instanceof Error ? error.message : undefined,
          tone: "danger",
        });
      });
  }

  return (
    <Dialog
      title="Spawn child session"
      size="sm"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" disabled={starting()} onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="primary" disabled={blocked()} onClick={start}>
            {starting() ? "Starting…" : "Spawn child"}
          </Button>
        </>
      }
    >
      <p class="panel-note">
        Starts a nested agent under <span class="tree-label">{props.request.title}</span>. Providers
        talk only through Forge — this is how one session asks another provider for help.
      </p>

      <Select
        label="Agent"
        value={chosen()}
        options={options()}
        onChange={setTarget}
        placeholder="Select an agent"
        disabled={options().length === 0}
      />
      <Show when={options().length === 0}>
        <p class="panel-note">No installed agent is available to launch.</p>
      </Show>

      <Select label="Role" value={role()} options={ROLE_OPTIONS} onChange={setRole} />
      <Select
        label="Workspace"
        value={workspace()}
        options={WORKSPACE_OPTIONS}
        onChange={setWorkspace}
      />

      <TextArea
        label="Initial prompt (optional)"
        value={prompt()}
        onChange={setPrompt}
        rows={4}
        placeholder="What should the child do?"
      />
    </Dialog>
  );
}
