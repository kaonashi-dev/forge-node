import { Show, createMemo } from "solid-js";
import { forgeStore } from "../../state/forgeStore";
import { connectionStore } from "../../state/connection";
import { terminalStore } from "../../features/terminal/terminalStore";
import { activeWorkspace } from "../../state/workspace";
import { centerMode, codeOpen, currentViews } from "../../navigation/viewsStore";
import { activeEditorPath } from "../../navigation/views";
import { editorFooter } from "../../features/editor/editorFooter";
import type { Workspace } from "../../contracts/runtime";

/** What the working tree says, in words. Unmeasured is not clean. */
function treeLabel(workspace: Workspace): string | null {
  const status = workspace.status;
  if (!status.measured_at) return null;
  return status.dirty ? "uncommitted changes" : "no changes";
}

function syncLabel(workspace: Workspace): string | null {
  const { ahead, behind } = workspace.status;
  if (!ahead && !behind) return null;
  return `↑${ahead ?? 0} ↓${behind ?? 0}`;
}

/** Location and status for whatever the card is showing: project / checkout / path. */
export function CardFooter() {
  const session = createMemo(
    () => forgeStore.sessions.find((item) => item.id === connectionStore.activeSession) ?? null,
  );
  const workspace = createMemo(() => {
    const id = session()?.workspace_id ?? activeWorkspace();
    return forgeStore.workspaces.find((item) => item.id === id) ?? null;
  });
  const project = createMemo(() => {
    const id = workspace()?.project_id;
    return forgeStore.projects.find((item) => item.id === id) ?? null;
  });
  const onCode = () => centerMode() === "code" && codeOpen();
  const path = () => {
    const file = onCode() ? activeEditorPath(currentViews().active) : null;
    const root = workspace()?.path;
    return file && root && file.startsWith(`${root}/`) ? file.slice(root.length + 1) : file;
  };
  const provider = createMemo(() => {
    const id = session()?.agent_provider_id;
    if (!id) return null;
    const found = forgeStore.providers.find((item) => item.descriptor?.id === id);
    return found?.descriptor?.display_name ?? id;
  });

  return (
    <footer class="card-footer">
      <Show when={project()}>
        {(item) => <span class="card-footer-place forge-truncate">{item().name}</span>}
      </Show>
      <Show when={workspace()}>
        {(item) => (
          <>
            <span class="card-footer-sep" aria-hidden="true">
              /
            </span>
            <span class="card-footer-mono card-footer-strong forge-truncate">
              {item().branch ?? item().display_name ?? item().path}
            </span>
            <Show when={path()}>
              {(file) => (
                <>
                  <span class="card-footer-sep" aria-hidden="true">
                    /
                  </span>
                  <span class="card-footer-mono forge-truncate">{file()}</span>
                </>
              )}
            </Show>
            <Show when={syncLabel(item())}>
              {(sync) => <span class="card-footer-mono card-footer-strong">{sync()}</span>}
            </Show>
            <Show when={treeLabel(item())}>
              {(tree) => (
                <>
                  <span class="card-footer-sep" aria-hidden="true">
                    ·
                  </span>
                  <span>{tree()}</span>
                </>
              )}
            </Show>
          </>
        )}
      </Show>
      <span class="card-footer-spacer" />
      <Show when={onCode() && editorFooter()}>
        {(facts) => (
          <>
            <Show when={facts().position}>
              {(position) => <span class="card-footer-mono card-footer-strong">{position()}</span>}
            </Show>
            <span class="card-footer-mono">
              {[...facts().details, facts().mark].filter(Boolean).join(" · ")}
            </span>
          </>
        )}
      </Show>
      <Show when={!onCode()}>
        <Show when={provider()}>{(name) => <span class="forge-truncate">{name()}</span>}</Show>
        <Show when={session() && terminalStore.cols > 0}>
          <span class="card-footer-mono">
            {terminalStore.cols} × {terminalStore.rows}
          </span>
        </Show>
      </Show>
    </footer>
  );
}
