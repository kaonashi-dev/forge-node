import { For, Show, createMemo, createSignal } from "solid-js";
import { newAgent, refreshSnapshot } from "../runtime/api";
import type { ExternalAgentSession } from "../runtime/types";
import { historyMenuItems } from "../shell/historyMenuItems";
import { forgeStore } from "../store/forgeStore";
import { requestConfirm } from "../store/runtimeStore";
import { workbenchStore } from "../store/workbenchStore";
import { Icon, SessionGlyph } from "../theme/icons";
import { Button, FilterHeader, ListCard, Menu, Tooltip, toast } from "../ui";
import { deleteExternalSession } from "../workbench/api";

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

  /** The profile that recorded a run, when it still exists. */
  function account(session: ExternalAgentSession) {
    if (!session.profile_id) return null;
    return forgeStore.agent_profiles.find((item) => item.id === session.profile_id) ?? null;
  }

  /**
   * Whether the account that recorded this run can be launched.
   *
   * The account, not the provider: a run recorded under a profile's config
   * directory only exists for a CLI started with that same directory, so the
   * row that matters is that profile's — which is also the one that stays
   * launchable when the provider is only reachable through a profile's own
   * executable.
   */
  function canResume(session: ExternalAgentSession): boolean {
    return forgeStore.launchables.some(
      (item) =>
        item.provider === session.provider && item.profile === session.profile_id && item.enabled,
    );
  }

  function blockedReason(session: ExternalAgentSession): string | null {
    if (canResume(session)) return null;
    if (session.profile_id && !account(session)) {
      return "The profile that recorded this run no longer exists";
    }
    return `${session.provider} is not installed`;
  }

  function resume(session: ExternalAgentSession): void {
    // The ordinary agent launch, told which conversation to re-enter — there
    // is no second kind of agent, and the provider's own CLI is what resumes.
    // The profile travels with it: the transcript lives in that account's
    // config directory, and the default account has never heard of its id.
    void newAgent(
      session.provider,
      session.profile_id,
      session.workspace_id,
      session.session_id,
    ).catch(() => undefined);
  }

  /**
   * The description names what is *not* lost: someone reading "delete" in a
   * tool that manages git worktrees has every reason to fear the checkout.
   */
  function confirmDelete(session: ExternalAgentSession): void {
    requestConfirm({
      title: `Delete "${session.title}"?`,
      description:
        `Removes ${session.provider}'s transcript for this run from disk. ` +
        "The conversation cannot be resumed or read afterwards. " +
        "Your code and branches are untouched.",
      confirmLabel: "Delete",
      destructive: true,
      onConfirm: () => {
        void deleteExternalSession(session.session_id, session.provider, session.profile_id)
          // History reaches the GUI only on the snapshot: there is no
          // `ExternalAgentsChanged` event to wait for.
          .then(() => refreshSnapshot())
          .catch((error: unknown) =>
            toast({
              title: "Could not delete the transcript.",
              detail: error instanceof Error ? error.message : undefined,
              tone: "danger",
            }),
          );
      },
    });
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
                <Show when={account(session)}>
                  {(profile) => <span class="history-account">{profile().name}</span>}
                </Show>
                <span>{session.message_count} turns</span>
                <Show when={session.subagent_count > 0}>
                  <span>{session.subagent_count} subagents</span>
                </Show>
              </>
            }
            actions={
              <>
                <Tooltip label={blockedReason(session) ?? "Re-enter this conversation"}>
                  <Button
                    variant="secondary"
                    size="xs"
                    disabled={Boolean(blockedReason(session))}
                    onClick={() => resume(session)}
                  >
                    Resume
                  </Button>
                </Tooltip>
                <Menu
                  triggerClass="history-more"
                  triggerLabel={`More actions for ${session.title}`}
                  trigger={<Icon name="more-horizontal" size={14} />}
                  items={historyMenuItems(session, () => confirmDelete(session))}
                />
              </>
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
