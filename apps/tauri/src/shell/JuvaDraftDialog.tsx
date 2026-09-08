import { createSignal, Show } from "solid-js";
import type { JuvaDraft, JuvaKind } from "../workbench/types";
import { juvaAsEditable } from "../workbench/types";
import { applyJuvaDraft } from "../workbench/api";
import { Button, Dialog, TextArea } from "../ui";

export type JuvaDraftDialogProps = {
  workspace: string;
  draft: JuvaDraft;
  onDismiss: () => void;
  onApplied?: () => void;
};

function labels(kind: JuvaKind): { title: string; confirm: string } {
  if (kind === "CommitMessage") return { title: "Commit", confirm: "Create commit" };
  if (kind === "PullRequest") return { title: "Open PR", confirm: "Push & open PR" };
  return { title: "Draft", confirm: "Apply" };
}

/**
 * Editable Juva draft before commit or PR (§16.5).
 */
export function JuvaDraftDialog(props: JuvaDraftDialogProps) {
  const [text, setText] = createSignal(juvaAsEditable(props.draft));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const meta = () => labels(props.draft.kind);
  const summary = () => {
    const ctx = props.draft.context;
    const files = ctx.files.length;
    const branch = ctx.branch ?? "(detached)";
    const truncated = ctx.truncated ? " · diff truncated" : "";
    return `${files} file(s) · branch ${branch}${truncated}`;
  };

  function apply(): void {
    setBusy(true);
    setError(null);
    const edited = text().trim();
    const parts = edited.split("\n\n");
    const title = parts[0]?.trim() ?? "";
    const body = parts.slice(1).join("\n\n").trim();
    void applyJuvaDraft(props.workspace, props.draft.kind, title, body)
      .then(() => {
        props.onApplied?.();
        props.onDismiss();
      })
      .catch(() => setError("The daemon refused to apply the draft."))
      .finally(() => setBusy(false));
  }

  return (
    <Dialog
      title={meta().title}
      description={summary()}
      size="lg"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy()} onClick={apply}>
            {meta().confirm}
          </Button>
        </>
      }
    >
      <TextArea
        class="juva-editor"
        aria-label={`${meta().title} message`}
        rows={12}
        value={text()}
        onChange={setText}
        onKeyDown={(event) => {
          if (!(event.metaKey || event.ctrlKey) || event.key !== "Enter") return;
          event.preventDefault();
          apply();
        }}
      />
      <Show when={error()}>{(message) => <p class="panel-error">{message()}</p>}</Show>
    </Dialog>
  );
}
