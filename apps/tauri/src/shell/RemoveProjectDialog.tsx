import { For, createMemo, createSignal } from "solid-js";
import { removeProject } from "../runtime/api";
import type { ProjectRemovalPolicy } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { setRuntimeStore } from "../store/runtimeStore";
import { AlertDialog, Button, RadioGroup } from "../ui";
import { describeProjectRemoval, policyIsRefused, projectFootprint } from "./projectRemoval";

export type RemoveProjectDialogProps = {
  project: string;
  onDismiss: () => void;
};

const POLICIES: { value: ProjectRemovalPolicy; label: string; note: string }[] = [
  {
    value: "keep_everything",
    label: "Keep everything",
    note: "Forget the project; nothing on disk changes",
  },
  {
    value: "kill_sessions",
    label: "Kill sessions",
    note: "Stop what is running, keep every checkout",
  },
  {
    value: "kill_sessions_and_worktrees",
    label: "Kill sessions and delete Forge Node's worktrees",
    note: "Only the worktrees Forge Node created are removed",
  },
];

/**
 * Removing a project, with the choice made before the request rather than
 * after a refusal.
 *
 * The worktree dialog can afford its two-step shape — ask, get refused, ask
 * again with `force` — because there is one thing to force. Here the daemon
 * takes three different policies and only rejects one of them, so a first
 * attempt would be an arbitrary guess at what the person meant to keep. The
 * policy that cannot be honoured is disabled with the reason next to it.
 */
export function RemoveProjectDialog(props: RemoveProjectDialogProps) {
  const project = createMemo(() => forgeStore.projects.find((item) => item.id === props.project));
  const name = createMemo(() => project()?.name ?? "this project");
  const footprint = createMemo(() => projectFootprint(forgeStore, props.project));
  const [policy, setPolicy] = createSignal<ProjectRemovalPolicy>("keep_everything");

  // The refused policy cannot be the one the dialog opens on: a person who
  // takes the default gets a notice instead of a removal.
  const chosen = createMemo<ProjectRemovalPolicy>(() =>
    policyIsRefused(policy(), footprint()) ? "kill_sessions" : policy(),
  );
  const consequences = createMemo(() => describeProjectRemoval(chosen(), footprint()));

  function confirm(): void {
    // Read before dismissing: `AppShell` mounts this from a non-keyed `<Show>`,
    // whose accessor throws `Stale read` the moment the condition goes falsy —
    // and `onDismiss` is what makes it falsy.
    const id = props.project;
    const selected = chosen();
    props.onDismiss();
    setRuntimeStore("notice", null);
    void removeProject(id, selected).catch(() => undefined);
  }

  return (
    <AlertDialog
      title={`Remove ${name()}?`}
      size="md"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="danger" onClick={confirm}>
            Remove project
          </Button>
        </>
      }
    >
      <p class="remove-worktree-copy forge-dialog-copy">
        {footprint().checkouts === 1
          ? "One checkout is registered under it."
          : `${footprint().checkouts} checkouts are registered under it.`}
      </p>

      <RadioGroup
        label="What to do with its sessions and worktrees"
        hideLabel={false}
        class="remove-project-policies"
        value={chosen()}
        onChange={(value) => setPolicy(value as ProjectRemovalPolicy)}
        options={POLICIES.map((item) => ({
          value: item.value,
          label: item.label,
          note: policyIsRefused(item.value, footprint())
            ? `Not available while ${
                footprint().runningSessions === 1
                  ? "a session is"
                  : `${footprint().runningSessions} sessions are`
              } running`
            : item.note,
          disabled: policyIsRefused(item.value, footprint()),
        }))}
      />

      <div class="remove-project-consequences">
        <p class="section-label">What happens</p>
        <ul class="remove-worktree-reasons">
          <For each={consequences()}>{(line) => <li>{line}</li>}</For>
        </ul>
      </div>
    </AlertDialog>
  );
}
