import { For, Show, createMemo } from "solid-js";
import { removeWorktree } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";
import { setRuntimeStore } from "../store/runtimeStore";
import { AlertDialog, Button } from "../ui";
import { describeWorktreeRemovalBlock } from "./worktreeRemoval";

export type RemoveWorktreeDialogProps = {
  workspace: string;
  reason: string | null;
  onDismiss: () => void;
};

/** Two-step removal: ordinary confirmation, then an explicit force if blocked. */
export function RemoveWorktreeDialog(props: RemoveWorktreeDialogProps) {
  const workspace = createMemo(() =>
    forgeStore.workspaces.find((item) => item.id === props.workspace),
  );
  const name = createMemo(() => {
    const item = workspace();
    return item?.display_name ?? item?.path.split("/").filter(Boolean).at(-1) ?? "this worktree";
  });
  const reasons = createMemo(() =>
    props.reason ? describeWorktreeRemovalBlock(props.reason) : [],
  );
  const managed = createMemo(() => workspace()?.managed_by_app !== false);

  function confirm(): void {
    // Read before dismissing, not after. `AppShell` mounts this from a
    // non-keyed `<Show when={runtimeStore.worktreeRemoval}>`, so `props`
    // resolves through that block's accessor — and the accessor throws
    // `Stale read from <Show>.` the moment the condition goes falsy.
    // `onDismiss` is what makes it falsy, so reading `props.workspace`
    // afterwards threw before `removeWorktree` was ever called: the dialog
    // closed and nothing else happened.
    const workspace = props.workspace;
    const force = props.reason !== null;
    props.onDismiss();
    setRuntimeStore("notice", null);
    void removeWorktree(workspace, force).catch(() => undefined);
  }

  return (
    <AlertDialog
      title={`Remove ${name()}?`}
      size="sm"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="danger" onClick={confirm}>
            {props.reason ? "Remove anyway" : "Remove worktree"}
          </Button>
        </>
      }
    >
      <Show
        when={props.reason}
        fallback={
          <p class="remove-worktree-copy forge-dialog-copy">
            {managed()
              ? "Forge Node will remove this worktree from disk. The branch and its commits are kept."
              : "Forge Node did not create this worktree, so it only forgets it: the directory and branch are kept, and Forge Node will not adopt it again."}
          </p>
        }
      >
        <p class="remove-worktree-copy forge-dialog-copy">
          Forge Node found conditions that make removal unsafe:
        </p>
        <ul class="remove-worktree-reasons">
          <For each={reasons()}>{(reason) => <li>{reason}</li>}</For>
        </ul>
        <p class="remove-worktree-copy forge-dialog-copy">
          {managed()
            ? "Removing it anyway discards the worktree and the state listed above. The branch is kept."
            : "Removing it anyway only forgets Forge Node's entry, permanently. The directory, branch, and local changes are kept."}
        </p>
      </Show>
    </AlertDialog>
  );
}
