import { For, Show, createMemo, createSignal } from "solid-js";
import { newAgent } from "../runtime/api";
import type { ExternalAgentSession } from "../runtime/types";
import { forgeStore } from "../store/forgeStore";
import { workbenchStore } from "../store/workbenchStore";
import { SessionGlyph } from "../theme/icons";
import { Button, FilterHeader, ListCard, Tooltip } from "../ui";

type Scope = "checkout" | "project" | "everywhere";

/**
 * Agent runs found on disk (§13.5).
 *
 * Read-only, and resumed by asking the provider's own CLI to re-enter its
 * session — never by replaying a transcript. A run whose provider is not
 * installed still lists: the transcript is the record either way.
 */
export function HistoryPanel() {
  const [scope, setScope] = createSignal<Scope>("checkout");
  const [query, setQuery] = createSignal("");

  const workspace = () =>
    forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace) ?? null;

  const sessions = createMemo(() => {
    const needle = query().trim().toLowerCase();
    const current = workspace();
    return (
      forgeStore.external_agents
        .filter((session) => {
          if (scope() === "checkout") return session.workspace_id === current?.id;
          if (scope() === "project") return session.project_id === current?.project_id;
          return true;
        })
        .filter((session) =>
          needle === ""
            ? true
            : `${session.title} ${session.branch ?? ""} ${session.provider}`
                .toLowerCase()
                .includes(needle),
        )
        // Most recent first: history is read from the top.
        .slice()
        .sort((a, b) => b.last_activity.localeCompare(a.last_activity))
    );
  });

  function providerInstalled(provider: string): boolean {
    return forgeStore.launchables.some(
      (item) => item.provider === provider && item.profile === null && item.enabled,
    );
  }

  function resume(session: ExternalAgentSession): void {
    // The ordinary agent launch, told which conversation to re-enter — there
    // is no second kind of agent, and the provider's own CLI is what resumes.
    void newAgent(session.provider, null, session.workspace_id, session.session_id).catch(
      () => undefined,
    );
  }

  return (
    <div class="panel-body">
      <FilterHeader
        label="Filter history"
        placeholder="Filter history…"
        query={query()}
        onQuery={setQuery}
        rows={[
          {
            label: "Scope",
            value: scope(),
            onChange: (value) => setScope(value as Scope),
            options: (["checkout", "project", "everywhere"] as Scope[]).map((value) => ({ value })),
          },
        ]}
      />
      <For each={sessions()} fallback={<p class="empty-copy">No agent runs recorded here.</p>}>
        {(session) => (
          <ListCard
            class="history-card"
            glyph={<SessionGlyph providerId={session.provider} size={14} />}
            title={session.title}
            aside={<span class="history-provider">{session.provider}</span>}
            meta={
              <>
                <Show when={session.branch}>{(branch) => <span>{branch()}</span>}</Show>
                <span>{session.message_count} turns</span>
                <Show when={session.subagent_count > 0}>
                  <span>{session.subagent_count} subagents</span>
                </Show>
              </>
            }
            actions={
              <Tooltip
                label={
                  providerInstalled(session.provider)
                    ? "Re-enter this conversation"
                    : `${session.provider} is not installed`
                }
              >
                <Button
                  variant="secondary"
                  size="xs"
                  disabled={!providerInstalled(session.provider)}
                  onClick={() => resume(session)}
                >
                  Resume
                </Button>
              </Tooltip>
            }
          >
            <Show when={session.preview}>
              {(preview) => <p class="history-preview">{preview()}</p>}
            </Show>
          </ListCard>
        )}
      </For>
    </div>
  );
}
