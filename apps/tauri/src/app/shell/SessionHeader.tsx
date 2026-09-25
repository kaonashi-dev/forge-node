import { Show, createMemo } from "solid-js";
import { forgeStore } from "../../state/forgeStore";
import { connectionStore } from "../../state/connection";
import { now } from "../../runtime/clock";
import { SessionGlyph } from "../../features/sessions/SessionGlyph";
import { SessionActionBar } from "../../features/sessions/SessionActionBar";
import { sessionTabLabel } from "../../features/sessions/attention";
import { sessionsInWorkspace } from "../../features/sessions/sessionScope";
import { hasQuestion } from "../../features/sessions/waiting";
import { elapsedLabel, since } from "../../features/sessions/elapsed";
import type { Session } from "../../contracts/runtime";
import { Icon } from "../../theme/icons/index";
import { IconButton } from "../../ui/index";

function rowFor(id: string | null | undefined): Session | null {
  const session = id ?? connectionStore.activeSession;
  return forgeStore.sessions.find((item) => item.id === session) ?? null;
}

/** Where the session on screen lives, what runs it, and whether it is waiting. */
export function SessionHeader(props: { sessionId?: string; onCloseSplit?: () => void }) {
  const session = createMemo(() => rowFor(props.sessionId));
  const workspace = createMemo(() => {
    const id = session()?.workspace_id;
    return forgeStore.workspaces.find((item) => item.id === id) ?? null;
  });
  const project = createMemo(() => {
    const id = workspace()?.project_id;
    return forgeStore.projects.find((item) => item.id === id) ?? null;
  });
  const label = () => {
    const current = session();
    if (!current) return "";
    const strip = sessionsInWorkspace(forgeStore.sessions, current.workspace_id).filter(
      (item) => item.terminal_id !== null,
    );
    return sessionTabLabel(current, strip);
  };
  const provider = createMemo(() => {
    const id = session()?.agent_provider_id;
    if (!id) return null;
    const found = forgeStore.providers.find((item) => item.descriptor?.id === id);
    return found?.descriptor?.display_name ?? id;
  });
  const waitingFor = () => {
    const current = session();
    if (!current || !hasQuestion(current.id)) return null;
    return elapsedLabel(since(current.last_activity_at ?? current.created_at, now()) ?? 0);
  };

  return (
    <Show when={session()}>
      {(current) => (
        <header class="session-header">
          <nav class="session-crumbs" aria-label="Session location">
            <Show when={project()}>
              {(item) => (
                <>
                  <span class="session-crumb">{item().name}</span>
                  <span class="session-crumb-sep" aria-hidden="true">
                    /
                  </span>
                </>
              )}
            </Show>
            <Show when={workspace()}>
              {(item) => (
                <>
                  <span class="session-crumb session-crumb-mono">
                    {item().display_name ?? item().branch ?? item().path}
                  </span>
                  <span class="session-crumb-sep" aria-hidden="true">
                    /
                  </span>
                </>
              )}
            </Show>
            <h2 class="session-crumb-title">{label()}</h2>
          </nav>
          <Show when={provider()}>
            {(name) => (
              <span class="session-chip">
                <SessionGlyph providerId={current().agent_provider_id} size={11} />
                <span>{name()}</span>
              </span>
            )}
          </Show>
          <Show when={waitingFor()}>
            {(elapsed) => (
              <span class="session-chip session-chip-waiting">
                <span class="forge-attention-dot" aria-hidden="true" />
                Waiting for you · {elapsed()}
              </span>
            )}
          </Show>
          <span class="session-header-spacer" />
          <Show when={!props.sessionId}>
            <SessionActionBar />
          </Show>
          <Show when={props.onCloseSplit}>
            <IconButton label="Join panes" size="xs" onClick={props.onCloseSplit}>
              <Icon name="close" class="forge-icon-muted" size={14} />
            </IconButton>
          </Show>
        </header>
      )}
    </Show>
  );
}
