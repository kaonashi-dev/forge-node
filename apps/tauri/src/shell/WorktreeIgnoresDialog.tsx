import { For, Show, createMemo } from "solid-js";
import { setWorktreeIgnores } from "../runtime/api";
import type { WorktreeIgnore } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { Button, Dialog } from "../ui";

export type WorktreeIgnoresDialogProps = {
  projectId: string;
  name: string;
  onDismiss: () => void;
};

/**
 * The rules that keep worktrees out of the rail, with a way to undo each one.
 *
 * A rule never touches the disk, so removing one here only asks the daemon to
 * rescan: the worktree is adopted again the next time git lists it.
 */
export function WorktreeIgnoresDialog(props: WorktreeIgnoresDialogProps) {
  const rules = createMemo(() =>
    forgeStore.worktree_ignores.filter((rule) => rule.project_id === props.projectId),
  );

  function stop(rule: WorktreeIgnore): void {
    // The daemon replaces the whole set, so send what should remain.
    void setWorktreeIgnores(
      props.projectId,
      rules().filter((item) => item.path !== rule.path),
    ).catch(() => undefined);
  }

  return (
    <Dialog
      title={`Ignored worktrees · ${props.name}`}
      size="md"
      onDismiss={props.onDismiss}
      footer={
        <Button variant="secondary" onClick={props.onDismiss}>
          Done
        </Button>
      }
    >
      <Show
        when={rules().length > 0}
        fallback={
          <p class="forge-dialog-copy">
            Forge Node adopts every worktree this repository lists, wherever it lives.
          </p>
        }
      >
        <p class="forge-dialog-copy">
          Forge Node never adopts a worktree at these paths. Nothing on disk was changed, so
          stopping here lets the next scan find them again.
        </p>
        <ul class="worktree-ignores">
          <For each={rules()}>
            {(rule) => (
              <li class="worktree-ignore">
                <div class="worktree-ignore-what">
                  <code class="worktree-ignore-path">{rule.path}</code>
                  <span class="worktree-ignore-scope">
                    {rule.scope === "subtree"
                      ? "This folder and every worktree under it"
                      : "This worktree only"}
                  </span>
                </div>
                <Button variant="secondary" onClick={() => stop(rule)}>
                  Stop ignoring
                </Button>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Dialog>
  );
}
